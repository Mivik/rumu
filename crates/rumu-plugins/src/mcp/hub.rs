//! The connections to the MCP servers in `settings.yaml`: one [`Server`] per
//! entry, connected in the background, its tools cached until the server
//! says they changed.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, JsonObject,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceTemplate, Tool,
};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ClientHandler, RoleClient, ServiceError, serve_client};
use rumu_core::{McpServerInfo, McpServerSettings, McpStatus, McpToolInfo};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tracing::{info, warn};

/// How long connecting (spawn or HTTP, `initialize`, `tools/list`) may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Stderr lines of a stdio server kept for its error message.
const STDERR_TAIL: usize = 20;

type Client = RunningService<RoleClient, Handler>;

/// Every configured server, by name.
pub struct McpHub {
    home: PathBuf,
    servers: Mutex<BTreeMap<String, Arc<Server>>>,
    changed: Arc<watch::Sender<u64>>,
}

impl McpHub {
    pub fn new(home: &Path) -> Arc<Self> {
        Arc::new(Self {
            home: home.to_owned(),
            servers: Mutex::default(),
            changed: Arc::new(watch::channel(0).0),
        })
    }

    /// Ticks whenever a server's status or tools change.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Match the servers to `settings`: drop the removed ones, and connect,
    /// in the background, the new and the changed ones.
    pub fn configure(&self, settings: &BTreeMap<String, McpServerSettings>) {
        let mut servers = self.servers.lock().expect("mcp servers lock");
        servers.retain(|name, s| settings.get(name) == Some(&s.settings));
        for (name, s) in settings {
            if !servers.contains_key(name) {
                let server = Arc::new(Server::new(name, s.clone(), &self.home, &self.changed));
                spawn_connect(&server);
                servers.insert(name.clone(), server);
            }
        }
        notify(&self.changed);
    }

    /// Drop server `name`'s connection and connect again.
    pub fn reconnect(&self, name: &str) {
        if let Some(server) = self.server(name) {
            spawn_connect(&server);
        }
    }

    pub fn server(&self, name: &str) -> Option<Arc<Server>> {
        self.servers
            .lock()
            .expect("mcp servers lock")
            .get(name)
            .cloned()
    }

    /// The web's view of `settings`, with each server's status.
    pub fn info(&self, settings: &BTreeMap<String, McpServerSettings>) -> Vec<McpServerInfo> {
        settings
            .iter()
            .map(|(name, s)| {
                let snap = self.server(name).map(|s| s.snapshot());
                McpServerInfo {
                    name: name.clone(),
                    command: s.command.clone(),
                    url: s.url.clone(),
                    env: s.env.keys().cloned().collect(),
                    headers: s.headers.keys().cloned().collect(),
                    status: snap.as_ref().map_or(McpStatus::Connecting, |s| s.status),
                    error: snap.as_ref().and_then(|s| s.error.clone()),
                    tools: snap
                        .map(|s| {
                            s.tools
                                .iter()
                                .map(|t| McpToolInfo {
                                    name: t.name.to_string(),
                                    description: t.description.as_deref().map(str::to_owned),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect()
    }
}

fn notify(changed: &watch::Sender<u64>) {
    changed.send_modify(|n| *n = n.wrapping_add(1));
}

fn spawn_connect(server: &Arc<Server>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let server = server.clone();
    tokio::spawn(async move { server.connect().await });
}

/// What is known of a server at one moment.
#[derive(Clone)]
pub struct Snapshot {
    pub status: McpStatus,
    pub error: Option<String>,
    pub tools: Arc<[Tool]>,
    pub instructions: Option<String>,
}

/// What a server lists of its resources.
#[derive(Default)]
pub struct Resources {
    pub resources: Vec<Resource>,
    pub templates: Vec<ResourceTemplate>,
}

struct State {
    snap: Snapshot,
    client: Option<Arc<Client>>,
}

/// One configured server and its connection.
pub struct Server {
    name: String,
    settings: McpServerSettings,
    home: PathBuf,
    state: Mutex<State>,
    /// Held while connecting, so connects do not overlap.
    connecting: tokio::sync::Mutex<()>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    changed: Arc<watch::Sender<u64>>,
}

impl Server {
    fn new(
        name: &str,
        settings: McpServerSettings,
        home: &Path,
        changed: &Arc<watch::Sender<u64>>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            settings,
            home: home.to_owned(),
            state: Mutex::new(State {
                snap: Snapshot {
                    status: McpStatus::Connecting,
                    error: None,
                    tools: Arc::new([]),
                    instructions: None,
                },
                client: None,
            }),
            connecting: tokio::sync::Mutex::new(()),
            stderr: Arc::default(),
            changed: changed.clone(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().expect("mcp state lock").snap.clone()
    }

    fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().expect("mcp state lock"));
        notify(&self.changed);
    }

    /// Connect, replacing any connection.
    async fn connect(self: Arc<Self>) {
        let _held = self.connecting.lock().await;
        self.establish().await;
    }

    /// Connect, replacing any connection; the caller holds `connecting`.
    async fn establish(self: &Arc<Self>) {
        self.set(|s| {
            s.snap.status = McpStatus::Connecting;
            s.snap.error = None;
            s.client = None;
        });
        let opened = tokio::time::timeout(CONNECT_TIMEOUT, self.open()).await;
        let opened = match opened {
            Ok(r) => r,
            Err(_) => Err(format!("no answer within {}s", CONNECT_TIMEOUT.as_secs())),
        };
        match opened {
            Ok((client, tools)) => {
                info!(server = self.name, tools = tools.len(), "mcp server ready");
                let instructions = client.peer_info().and_then(|i| i.instructions.clone());
                self.set(|s| {
                    s.snap = Snapshot {
                        status: McpStatus::Ready,
                        error: None,
                        tools: tools.into(),
                        instructions,
                    };
                    s.client = Some(Arc::new(client));
                });
            }
            Err(e) => {
                let tail: Vec<String> = self
                    .stderr
                    .lock()
                    .expect("stderr lock")
                    .iter()
                    .cloned()
                    .collect();
                let error = if tail.is_empty() {
                    e
                } else {
                    format!("{e}\n{}", tail.join("\n"))
                };
                warn!(server = self.name, error, "mcp server failed");
                self.set(|s| {
                    s.snap.status = McpStatus::Failed;
                    s.snap.error = Some(error);
                    s.snap.tools = Arc::new([]);
                });
            }
        }
    }

    async fn open(self: &Arc<Self>) -> Result<(Client, Vec<Tool>), String> {
        let handler = Handler {
            server: Arc::downgrade(self),
        };
        let client = if let Some((program, args)) = self.settings.command.split_first() {
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(args)
                .envs(&self.settings.env)
                .current_dir(&self.home);
            let (proc, stderr) = TokioChildProcess::builder(cmd)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("starting `{program}`: {e}"))?;
            self.stderr.lock().expect("stderr lock").clear();
            if let Some(stderr) = stderr {
                let (tail, name) = (self.stderr.clone(), self.name.clone());
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!(server = name, "{line}");
                        let mut tail = tail.lock().expect("stderr lock");
                        if tail.len() == STDERR_TAIL {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                });
            }
            serve_client(handler, proc)
                .await
                .map_err(|e| e.to_string())?
        } else if let Some(url) = &self.settings.url {
            let mut headers = std::collections::HashMap::new();
            for (k, v) in &self.settings.headers {
                let name = k
                    .parse()
                    .map_err(|_| format!("`{k}` is not a valid header name"))?;
                let value = v
                    .parse()
                    .map_err(|_| format!("header `{k}` has an invalid value"))?;
                headers.insert(name, value);
            }
            let config =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(headers);
            let transport = StreamableHttpClientTransport::from_config(config);
            serve_client(handler, transport)
                .await
                .map_err(|e| e.to_string())?
        } else {
            return Err("neither a command nor a URL is set".into());
        };
        let tools = client.list_all_tools().await.map_err(|e| e.to_string())?;
        Ok((client, tools))
    }

    /// The tools, once any connect in flight has finished.
    pub async fn ready(&self) -> Snapshot {
        let mut changed = self.changed.subscribe();
        let wait = async {
            loop {
                let snap = self.snapshot();
                if snap.status != McpStatus::Connecting {
                    return;
                }
                if changed.changed().await.is_err() {
                    return;
                }
            }
        };
        let _ = tokio::time::timeout(CONNECT_TIMEOUT + Duration::from_secs(5), wait).await;
        self.snapshot()
    }

    /// The connection, connecting again when it is gone. The check and the
    /// connect happen under `connecting`, so concurrent callers that find
    /// it gone reconnect once, not once each.
    async fn client(self: &Arc<Self>) -> Result<Arc<Client>, String> {
        self.ready().await;
        let _held = self.connecting.lock().await;
        if let Some(c) = self.live() {
            return Ok(c);
        }
        self.establish().await;
        let s = self.state.lock().expect("mcp state lock");
        s.client.clone().ok_or_else(|| {
            s.snap
                .error
                .clone()
                .unwrap_or_else(|| "not connected".into())
        })
    }

    fn live(&self) -> Option<Arc<Client>> {
        let s = self.state.lock().expect("mcp state lock");
        s.client.clone().filter(|c| !c.is_transport_closed())
    }

    /// Call `tool`. A call is never retried: when the connection drops
    /// mid-call the tool may have run, so the error goes to the model and
    /// the next call reconnects. Dropping the future (an abort) sends
    /// `notifications/cancelled`.
    pub async fn call(self: &Arc<Self>, tool: &str, args: JsonObject) -> Result<Value, String> {
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(args);
        let result = self
            .request(|c| async move { c.call_tool(params).await })
            .await?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    /// `resources/read` of `uri`.
    pub async fn read_resource(self: &Arc<Self>, uri: &str) -> Result<ReadResourceResult, String> {
        let params = ReadResourceRequestParams::new(uri);
        self.request(|c| async move { c.read_resource(params).await })
            .await
    }

    /// Its resources and resource templates, every page; none when it does
    /// not declare `resources` (nor templates when it does not serve them).
    pub async fn resources(self: &Arc<Self>) -> Result<Resources, String> {
        self.request(|c| async move {
            if !c
                .peer_info()
                .is_some_and(|i| i.capabilities.resources.is_some())
            {
                return Ok(Resources::default());
            }
            let resources = c.list_all_resources().await?;
            let templates = match c.list_all_resource_templates().await {
                Err(ServiceError::McpError(_)) => Vec::new(),
                r => r?,
            };
            Ok(Resources {
                resources,
                templates,
            })
        })
        .await
    }

    /// Run `f` on the connection, connecting first when it is gone. When
    /// the connection drops on the way, the next request reconnects.
    async fn request<T, F>(self: &Arc<Self>, f: impl FnOnce(Arc<Client>) -> F) -> Result<T, String>
    where
        F: Future<Output = Result<T, ServiceError>>,
    {
        let client = self.client().await?;
        let result = f(client.clone()).await;
        if let Err(ServiceError::TransportClosed | ServiceError::TransportSend(_)) = &result {
            warn!(server = self.name, "mcp connection dropped");
            let mut s = self.state.lock().expect("mcp state lock");
            if s.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client)) {
                s.client = None;
            }
        }
        result.map_err(|e| e.to_string())
    }

    async fn refresh_tools(self: Arc<Self>) {
        let Some(client) = self.state.lock().expect("mcp state lock").client.clone() else {
            return;
        };
        match client.list_all_tools().await {
            Ok(tools) => self.set(|s| s.snap.tools = tools.into()),
            Err(e) => warn!(server = self.name, error = %e, "listing mcp tools failed"),
        }
    }
}

/// The client side of a connection: refreshes the tools when told they
/// changed.
pub struct Handler {
    server: Weak<Server>,
}

impl ClientHandler for Handler {
    async fn on_tool_list_changed(&self, _cx: NotificationContext<RoleClient>) {
        if let Some(server) = self.server.upgrade() {
            tokio::spawn(server.refresh_tools());
        }
    }

    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("rumu", env!("CARGO_PKG_VERSION")),
        )
    }
}
