//! `RUMU_HOME/settings.yaml`: what the web's settings dialog edits. rumu is
//! its only writer. The file holds API keys, so it is written owner-only.
//!
//! ```yaml
//! providers:
//!   deepseek:
//!     preset: deepseek          # its endpoints, and its catalog entry
//!                               # (a built-in, else a catalog provider)
//!     api_key: …
//!     models: [deepseek-*]      # patterns over the listed ids, or ids
//!   gateway:
//!     endpoints: { anthropic: https://gw.example, openai: https://gw.example/v1 }
//!     api_key: …
//!     models: [claude-*, gpt-5]
//!     pinned: [gpt-5]
//!     protocols: { gpt-5: openai }
//! custom:                       # the built-in provider's models: rumu/<name>
//!   main: { model: gateway/claude-opus-5-5, effort: high }
//!   cheap: { model: deepseek/deepseek-chat, effort: low }
//! roles:                        # model refs, custom or not
//!   default: rumu/main
//!   title: rumu/cheap
//! titles: model                 # session titles: first, last or model
//! ```
//!
//! The ids each provider lists are fetched from its endpoints and cached at
//! `RUMU_HOME/cache/providers/<id>.json`; what is known about models comes
//! from models.dev, cached at `RUMU_HOME/cache/models.json`. Both are
//! refreshed daily in the background.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use rumu_core::{
    CUSTOM_PROVIDER, CustomModel, Effort, IoError, ModelConfig, ModelInfo, PresetInfo, Protocol,
    ProviderInfo, ProviderSettings, Roles, SettingsInfo, Titles, enabled_models, split_model_ref,
};
use rumu_kernel::LlmError;
use rumu_llm::{Catalog, GenaiLlm, endpoints, model_info, presets};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

const SETTINGS_FILE: &str = "settings.yaml";
const CATALOG_URL: &str = "https://models.dev/api.json";
const CATALOG_CACHE: &str = "cache/models.json";
const LISTS_CACHE: &str = "cache/providers";
/// A cached catalog or model list older than this is refreshed at startup.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Io(#[from] IoError),

    #[error("{path}: {message}")]
    Yaml { path: PathBuf, message: String },

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error("{0}")]
    Invalid(String),

    #[error("fetching the model catalog: {0}")]
    Catalog(String),
}

type Result<T> = std::result::Result<T, SettingsError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    #[serde(flatten)]
    model: ModelConfig,
}

/// A `settings.set_provider` request. The web never sees stored keys, so
/// `api_key` is only sent to replace it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEdit {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    endpoints: BTreeMap<Protocol, String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    protocols: BTreeMap<String, Protocol>,
}

impl ProviderEdit {
    pub fn id(&self) -> &str {
        self.id.trim()
    }
}

/// Refresh, in the background, the catalog (when `catalog`) and the model
/// lists of providers `lists`, then push the settings. The settings work
/// without either (unknown models get the protocol's defaults; exact ids
/// need no list).
pub fn spawn_refresh(
    settings: &Arc<Settings>,
    kernel: &Arc<rumu_kernel::Kernel>,
    catalog: bool,
    lists: Vec<String>,
) {
    if !catalog && lists.is_empty() {
        return;
    }
    let (settings, kernel) = (settings.clone(), kernel.clone());
    tokio::spawn(async move {
        if catalog && let Err(e) = settings.refresh_catalog().await {
            warn!(error = %e, "model catalog unavailable");
        }
        for id in lists {
            if let Err(e) = settings.refresh_models(&id).await {
                warn!(provider = id, error = %e, "listing models failed");
            }
        }
        kernel.settings_changed(serde_json::json!(settings.info()));
    });
}

/// A provider's last model list.
#[derive(Debug, Clone, Default)]
struct Listing {
    ids: Vec<String>,
    error: Option<String>,
}

/// A `settings.set_custom` request; `rename` replaces the custom model of
/// that name, and roles bound to it follow.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomEdit {
    name: String,
    #[serde(default)]
    rename: Option<String>,
    model: String,
    #[serde(default)]
    effort: Option<Effort>,
}

/// The loaded settings, applied to the model adapter they configure.
pub struct Settings {
    home: PathBuf,
    file: Mutex<File>,
    catalog: RwLock<Arc<Catalog>>,
    listed: Mutex<BTreeMap<String, Listing>>,
    /// Each provider's [`ProviderInfo`], derived once from its settings, its
    /// listing and the catalog, and dropped when one of those changes.
    infos: Mutex<BTreeMap<String, ProviderInfo>>,
    llm: Arc<GenaiLlm>,
}

impl Settings {
    /// Read the file (missing is all defaults) and the cached catalog, and
    /// configure `llm` from them. An invalid file is reported and the
    /// adapter left unconfigured, so it can be fixed from the web.
    pub fn load(home: &Path, llm: Arc<GenaiLlm>) -> Result<Self> {
        let path = home.join(SETTINGS_FILE);
        let file: File = match std::fs::read_to_string(&path) {
            Ok(text) => rumu_cards::yaml::from_str::<Option<File>>(&text)
                .map_err(|e| SettingsError::Yaml {
                    path: path.clone(),
                    message: e.to_string(),
                })?
                .unwrap_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(IoError::at("reading", &path)(e).into()),
        };
        let catalog = Arc::new(read_cached_catalog(home).unwrap_or_default());
        if let Err(e) = llm.configure(&file.model, catalog.clone()) {
            warn!(error = %e, "model settings are invalid; fix them in Settings");
        }
        let listed = file
            .model
            .providers
            .keys()
            .map(|id| {
                let ids = std::fs::read(list_cache(home, id))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                (id.clone(), Listing { ids, error: None })
            })
            .collect();
        Ok(Self {
            home: home.to_owned(),
            file: Mutex::new(file),
            catalog: RwLock::new(catalog),
            listed: Mutex::new(listed),
            infos: Mutex::default(),
            llm,
        })
    }

    fn catalog(&self) -> Arc<Catalog> {
        self.catalog.read().expect("catalog lock").clone()
    }

    pub fn info(&self) -> SettingsInfo {
        let file = self.lock();
        // Held while deriving, so no info of the replaced catalog is cached.
        let catalog = self.catalog.read().expect("catalog lock");
        let listed = self.listed.lock().expect("listed lock");
        let mut infos = self.infos.lock().expect("infos lock");
        let m = &file.model;
        SettingsInfo {
            providers: m
                .providers
                .iter()
                .map(|(id, p)| {
                    infos
                        .entry(id.clone())
                        .or_insert_with(|| provider_info(&catalog, id, p, listed.get(id)))
                        .clone()
                })
                .collect(),
            custom: m
                .custom
                .iter()
                .map(|(name, c)| custom_info(&catalog, m, name, c))
                .collect(),
            roles: m.roles.clone(),
            titles: m.titles,
            presets: offered(&catalog),
        }
    }

    pub fn set_provider(&self, edit: ProviderEdit) -> Result<SettingsInfo> {
        let id = edit.id().to_owned();
        if id.is_empty() || id.contains('/') {
            return Err(SettingsError::Invalid(format!(
                "provider id `{id}` must be non-empty and without `/`"
            )));
        }
        if id == CUSTOM_PROVIDER {
            return Err(SettingsError::Invalid(format!(
                "provider id `{id}` is the built-in provider's"
            )));
        }
        let mut models: Vec<String> = Vec::new();
        for m in edit.models {
            let m = m.trim().to_owned();
            if !m.is_empty() && !models.contains(&m) {
                models.push(m);
            }
        }
        let catalog = self.catalog();
        self.edit(|file| {
            let old = file.providers.remove(&id).unwrap_or_default();
            let name = match &edit.name {
                Some(n) => Some(n.trim()).filter(|n| !n.is_empty()).map(str::to_owned),
                None => old.name,
            };
            if let Some(p) = edit.preset.as_deref()
                && presets::preset(p).is_none()
                && catalog.provider(p).is_none()
                && old.preset.as_deref() != Some(p)
            {
                return Err(SettingsError::Invalid(format!(
                    "unknown provider preset `{p}`"
                )));
            }
            let api_key = match edit.api_key {
                Some(k) => k.trim().to_owned(),
                None => old.api_key,
            };
            file.providers.insert(
                id,
                ProviderSettings {
                    name,
                    endpoints: overrides(edit.preset.as_deref(), edit.endpoints),
                    preset: edit.preset,
                    api_key,
                    models,
                    pinned: old.pinned,
                    protocols: edit.protocols,
                },
            );
            Ok(())
        })
    }

    /// The endpoints `url` serves, asked with `api_key` (unset is provider
    /// `id`'s stored key).
    pub async fn probe(
        &self,
        url: &str,
        api_key: Option<String>,
        id: Option<&str>,
    ) -> rumu_llm::Probe {
        let key = api_key.unwrap_or_else(|| {
            id.and_then(|id| {
                self.lock()
                    .model
                    .providers
                    .get(id)
                    .map(|p| p.api_key.clone())
            })
            .unwrap_or_default()
        });
        rumu_llm::probe(url, key.trim()).await
    }

    /// What is known about `model_ref`, enabled or not.
    pub fn model(&self, model_ref: &str) -> Result<ModelInfo> {
        let (provider, model) = split_model_ref(model_ref)
            .ok_or_else(|| SettingsError::Invalid(format!("not a model ref: `{model_ref}`")))?;
        let file = self.lock();
        let m = &file.model;
        if provider == CUSTOM_PROVIDER {
            let c = m
                .custom
                .get(model)
                .ok_or_else(|| SettingsError::Invalid(format!("unknown custom model `{model}`")))?;
            return Ok(custom_info(&self.catalog(), m, model, c));
        }
        let p = m
            .providers
            .get(provider)
            .ok_or_else(|| SettingsError::Invalid(format!("unknown provider `{provider}`")))?;
        Ok(model_info(
            &self.catalog(),
            provider,
            p,
            &endpoints(p),
            model,
        ))
    }

    /// Pin or unpin `model_ref` in the picker.
    pub fn pin(&self, model_ref: &str, pinned: bool) -> Result<SettingsInfo> {
        let (provider, model) = split_model_ref(model_ref)
            .ok_or_else(|| SettingsError::Invalid(format!("not a model ref: `{model_ref}`")))?;
        self.edit(|file| {
            let p = file
                .providers
                .get_mut(provider)
                .ok_or_else(|| SettingsError::Invalid(format!("unknown provider `{provider}`")))?;
            p.pinned.retain(|m| m != model);
            if pinned {
                p.pinned.push(model.to_owned());
            }
            Ok(())
        })
    }

    pub fn remove_provider(&self, id: &str) -> Result<SettingsInfo> {
        let of = |r: &str| split_model_ref(r).is_some_and(|(p, _)| p == id);
        self.edit(|file| {
            if let Some((name, _)) = file.custom.iter().find(|(_, c)| of(&c.model)) {
                return Err(SettingsError::Invalid(format!(
                    "custom model `{name}` uses provider `{id}`"
                )));
            }
            if let Some(r) = [&file.roles.default, &file.roles.title]
                .into_iter()
                .flatten()
                .find(|r| of(r))
            {
                return Err(SettingsError::Invalid(format!("`{r}` is a default model")));
            }
            file.providers.remove(id);
            Ok(())
        })
        .inspect(|_| {
            self.listed.lock().expect("listed lock").remove(id);
            let _ = std::fs::remove_file(list_cache(&self.home, id));
        })
    }

    pub fn set_custom(&self, edit: CustomEdit) -> Result<SettingsInfo> {
        let name = edit.name.trim().to_owned();
        if name.is_empty() {
            return Err(SettingsError::Invalid("a custom model needs a name".into()));
        }
        self.edit(|file| {
            if let Some(old) = edit.rename.as_deref().filter(|o| *o != name) {
                if file.custom.contains_key(&name) {
                    return Err(SettingsError::Invalid(format!(
                        "custom model `{name}` already exists"
                    )));
                }
                file.custom.remove(old);
                let (old, new) = (custom_ref(old), custom_ref(&name));
                for slot in [&mut file.roles.default, &mut file.roles.title] {
                    if slot.as_deref() == Some(&old) {
                        *slot = Some(new.clone());
                    }
                }
            }
            file.custom.insert(
                name,
                CustomModel {
                    model: edit.model.trim().to_owned(),
                    effort: edit.effort,
                },
            );
            Ok(())
        })
    }

    pub fn remove_custom(&self, name: &str) -> Result<SettingsInfo> {
        self.edit(|file| {
            file.custom.remove(name);
            let gone = custom_ref(name);
            for slot in [&mut file.roles.default, &mut file.roles.title] {
                if slot.as_deref() == Some(&gone) {
                    *slot = None;
                }
            }
            Ok(())
        })
    }

    pub fn set_titles(&self, titles: Titles) -> Result<SettingsInfo> {
        self.edit(|file| {
            file.titles = titles;
            Ok(())
        })
    }

    pub fn set_roles(&self, roles: Roles) -> Result<SettingsInfo> {
        self.edit(|file| {
            file.roles = roles;
            Ok(())
        })
    }

    /// Apply `f` to a copy of the settings, configure the adapter with it
    /// (an invalid result is refused before anything is written), then save.
    fn edit(&self, f: impl FnOnce(&mut ModelConfig) -> Result<()>) -> Result<SettingsInfo> {
        {
            let mut file = self.lock();
            let mut next = file.clone();
            f(&mut next.model)?;
            let catalog = self.catalog();
            self.llm.configure(&next.model, catalog.clone())?;
            if let Err(e) = save(&self.home.join(SETTINGS_FILE), &next) {
                // Still the old settings on disk: run with those.
                let _ = self.llm.configure(&file.model, catalog);
                return Err(e);
            }
            let mut infos = self.infos.lock().expect("infos lock");
            infos.retain(|id, _| file.model.providers.get(id) == next.model.providers.get(id));
            *file = next;
        }
        Ok(self.info())
    }

    /// Fetch the catalog from models.dev, cache it, and re-apply the
    /// settings with it (efforts and API roots may have changed).
    pub async fn refresh_catalog(&self) -> Result<()> {
        let bytes = fetch_catalog().await?;
        let catalog = Catalog::parse(&bytes).map_err(|e| SettingsError::Catalog(e.to_string()))?;
        let cache = self.home.join(CATALOG_CACHE);
        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir).map_err(IoError::at("creating", dir))?;
        }
        std::fs::write(&cache, &bytes).map_err(IoError::at("writing", &cache))?;
        let catalog = Arc::new(catalog);
        {
            let mut current = self.catalog.write().expect("catalog lock");
            *current = catalog.clone();
            self.infos.lock().expect("infos lock").clear();
        }
        let file = self.lock();
        if let Err(e) = self.llm.configure(&file.model, catalog) {
            warn!(error = %e, "model settings are invalid; fix them in Settings");
        }
        info!("model catalog refreshed");
        Ok(())
    }

    /// Whether the cached catalog is missing or stale.
    pub fn catalog_stale(&self) -> bool {
        stale(&self.home.join(CATALOG_CACHE))
    }

    /// Fetch the ids provider `id` lists, and cache them. A failure is kept
    /// on the listing (the previous ids stay), not returned.
    pub async fn refresh_models(&self, id: &str) -> Result<()> {
        let (endpoints, key) = {
            let file = self.lock();
            let p = file
                .model
                .providers
                .get(id)
                .ok_or_else(|| SettingsError::Invalid(format!("unknown provider `{id}`")))?;
            (endpoints(p), p.api_key.clone())
        };
        let result = rumu_llm::list_models(&endpoints, &key).await;
        let mut listed = self.listed.lock().expect("listed lock");
        self.infos.lock().expect("infos lock").remove(id);
        let listing = listed.entry(id.to_owned()).or_default();
        match result {
            Ok(ids) => {
                let cache = list_cache(&self.home, id);
                if let Some(dir) = cache.parent() {
                    std::fs::create_dir_all(dir).map_err(IoError::at("creating", dir))?;
                }
                let json = serde_json::to_vec(&ids).expect("ids serialize");
                std::fs::write(&cache, json).map_err(IoError::at("writing", &cache))?;
                *listing = Listing { ids, error: None };
            }
            Err(e) => {
                warn!(provider = id, error = %e, "listing models failed");
                listing.error = Some(e);
            }
        }
        Ok(())
    }

    /// The providers whose cached model list is missing or stale.
    pub fn stale_lists(&self) -> Vec<String> {
        self.lock()
            .model
            .providers
            .keys()
            .filter(|id| stale(&list_cache(&self.home, id)))
            .cloned()
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.file.lock().expect("settings lock")
    }
}

fn provider_info(
    catalog: &Catalog,
    id: &str,
    p: &ProviderSettings,
    listing: Option<&Listing>,
) -> ProviderInfo {
    let listing = listing.cloned().unwrap_or_default();
    let eps = endpoints(p);
    let mut models: Vec<_> = enabled_models(&p.models, &listing.ids)
        .iter()
        .map(|m| model_info(catalog, id, p, &eps, m))
        .collect();
    models.sort_by_key(|m| !m.pinned);
    let home = p.preset.as_deref().map(presets::home);
    let names = listing
        .ids
        .iter()
        .map(|m| (m.clone(), catalog.name(home, m)))
        .collect();
    ProviderInfo {
        id: id.to_owned(),
        name: p
            .name
            .clone()
            .or_else(|| {
                p.preset
                    .as_deref()
                    .and_then(|x| preset_name(catalog, x))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| id.to_owned()),
        custom_name: p.name.clone(),
        preset: p.preset.clone(),
        endpoints: eps,
        has_api_key: !p.api_key.is_empty(),
        rules: p.models.clone(),
        protocols: p.protocols.clone(),
        models,
        listed: listing.ids,
        names,
        list_error: listing.error,
    }
}

fn custom_ref(name: &str) -> String {
    format!("{CUSTOM_PROVIDER}/{name}")
}

/// Custom model `name`: what is known of the model it routes to, under its
/// own ref and name.
fn custom_info(catalog: &Catalog, m: &ModelConfig, name: &str, c: &CustomModel) -> ModelInfo {
    let target = split_model_ref(&c.model).and_then(|(provider, id)| {
        let p = m.providers.get(provider)?;
        Some(model_info(catalog, provider, p, &endpoints(p), id))
    });
    let mut info = target.unwrap_or_else(|| {
        let none = ProviderSettings::default();
        model_info(catalog, CUSTOM_PROVIDER, &none, &BTreeMap::new(), name)
    });
    info.model_ref = custom_ref(name);
    info.id = name.to_owned();
    info.name = name.to_owned();
    info.pinned = false;
    info.route = Some(c.clone());
    info
}

/// The presets offered: the built-in ones, then the catalog's other
/// providers by name.
fn offered(catalog: &Catalog) -> Vec<PresetInfo> {
    let mut more: Vec<PresetInfo> = catalog
        .offered()
        .filter(|(id, _)| {
            !presets::PRESETS
                .iter()
                .any(|b| b.id == *id || b.catalog == *id)
        })
        .filter_map(|(id, p)| {
            let (protocol, url) = p.endpoint.clone()?;
            Some(PresetInfo {
                id: id.to_owned(),
                name: p.name.clone(),
                endpoints: BTreeMap::from([(protocol, url)]),
            })
        })
        .collect();
    more.sort_by_cached_key(|p| p.name.to_lowercase());
    presets::PRESETS
        .iter()
        .map(|p| PresetInfo {
            id: p.id.to_owned(),
            name: p.name.to_owned(),
            endpoints: p
                .endpoints
                .iter()
                .map(|(k, v)| (*k, (*v).to_owned()))
                .collect(),
        })
        .chain(more)
        .collect()
}

/// Preset `id`'s name: a built-in's, else the catalog provider's.
fn preset_name<'a>(catalog: &'a Catalog, id: &str) -> Option<&'a str> {
    presets::preset(id)
        .map(|p| p.name)
        .or_else(|| Some(catalog.provider(id)?.name.as_str()))
}

/// What a provider stores of the endpoints it is to use: those that differ
/// from its preset's, and an empty one for each of the preset's it drops.
fn overrides(
    preset: Option<&str>,
    wanted: BTreeMap<Protocol, String>,
) -> BTreeMap<Protocol, String> {
    let wanted: BTreeMap<Protocol, String> = wanted
        .into_iter()
        .map(|(k, v)| (k, v.trim().trim_end_matches('/').to_owned()))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let base = preset
        .and_then(presets::preset)
        .map(|p| p.endpoints)
        .unwrap_or_default();
    let mut out: BTreeMap<Protocol, String> = wanted
        .iter()
        .filter(|(k, v)| !base.iter().any(|(bk, bv)| bk == *k && bv == v))
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    for (k, _) in base {
        if !wanted.contains_key(k) {
            out.insert(*k, String::new());
        }
    }
    out
}

fn list_cache(home: &Path, id: &str) -> PathBuf {
    home.join(LISTS_CACHE).join(format!("{id}.json"))
}

/// Whether `path` is missing or older than [`MAX_AGE`].
fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_none_or(|age| age > MAX_AGE)
}

fn read_cached_catalog(home: &Path) -> Option<Catalog> {
    let bytes = std::fs::read(home.join(CATALOG_CACHE)).ok()?;
    Catalog::parse(&bytes)
        .inspect_err(|e| warn!(error = %e, "ignoring the cached model catalog"))
        .ok()
}

async fn fetch_catalog() -> Result<Vec<u8>> {
    let err = |e: reqwest::Error| SettingsError::Catalog(e.to_string());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(err)?;
    let resp = client
        .get(CATALOG_URL)
        .send()
        .await
        .map_err(err)?
        .error_for_status()
        .map_err(err)?;
    Ok(resp.bytes().await.map_err(err)?.to_vec())
}

/// Write the file whole, owner-only, through a temp file.
fn save(path: &Path, file: &File) -> Result<()> {
    let text = serde_saphyr::to_string(file).map_err(|e| SettingsError::Yaml {
        path: path.to_owned(),
        message: format!("serializing: {e}"),
    })?;
    let tmp = path.with_extension("yaml.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let write = |opts: &std::fs::OpenOptions| {
        use std::io::Write as _;
        opts.open(&tmp)?.write_all(text.as_bytes())
    };
    write(&opts).map_err(IoError::at("writing", &tmp))?;
    std::fs::rename(&tmp, path).map_err(IoError::at("renaming", &tmp))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, key: Option<&str>) -> ProviderEdit {
        ProviderEdit {
            id: id.into(),
            name: None,
            preset: Some("anthropic".into()),
            endpoints: BTreeMap::new(),
            api_key: key.map(Into::into),
            models: vec![" m ".into(), "m".into(), "n".into()],
            protocols: BTreeMap::new(),
        }
    }

    fn custom(name: &str, model: &str) -> CustomEdit {
        CustomEdit {
            name: name.into(),
            rename: None,
            model: model.into(),
            effort: Some(Effort::Low),
        }
    }

    #[test]
    fn edits_round_trip_and_invalid_ones_change_nothing() {
        let home = tempfile::tempdir().unwrap();
        let llm = Arc::new(GenaiLlm::default());
        let s = Settings::load(home.path(), llm.clone()).unwrap();

        let info = s.set_provider(provider("a", Some("k"))).unwrap();
        assert!(info.providers[0].has_api_key);
        assert_eq!(info.providers[0].models.len(), 2);
        assert_eq!(info.providers[0].name, "Anthropic");
        // No key keeps the stored one, and pins stay.
        s.pin("a/n", true).unwrap();
        let info = s.set_provider(provider("a", None)).unwrap();
        let ids: Vec<_> = info.providers[0]
            .models
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["n", "m"]);
        // A provider's info is derived once, and anew after its settings
        // change.
        s.set_titles(Titles::Last).unwrap();
        assert!(s.infos.lock().unwrap().contains_key("a"));
        let info = s.pin("a/m", true).unwrap();
        assert!(info.providers[0].models.iter().all(|m| m.pinned));
        s.pin("a/m", false).unwrap();
        // Endpoints are stored as what differs from the preset's.
        let mut edit = provider("a", None);
        edit.endpoints = [(Protocol::Openai, "https://gw.example/v1/".to_owned())].into();
        let info = s.set_provider(edit).unwrap();
        assert_eq!(info.providers[0].endpoints.len(), 1);
        let stored = s.lock().model.providers["a"].endpoints.clone();
        assert_eq!(stored[&Protocol::Anthropic], "");
        let mut edit = provider("a", None);
        edit.endpoints = [(Protocol::Anthropic, "https://api.anthropic.com".to_owned())].into();
        s.set_provider(edit).unwrap();
        assert!(s.lock().model.providers["a"].endpoints.is_empty());

        let mut bad = provider("b", None);
        bad.preset = Some("nowhere".into());
        assert!(s.set_provider(bad).is_err());

        // The built-in provider's id is taken.
        assert!(s.set_provider(provider(CUSTOM_PROVIDER, None)).is_err());

        // A custom model on an unknown provider, or a role on an unknown
        // model, is refused.
        assert!(matches!(
            s.set_custom(custom("main", "b/m")),
            Err(SettingsError::Llm(LlmError::UnknownModel(_)))
        ));
        let info = s.set_custom(custom("main", "a/m")).unwrap();
        let main = &info.custom[0];
        assert_eq!(
            (main.model_ref.as_str(), main.name.as_str()),
            ("rumu/main", "main")
        );
        assert_eq!(main.route.as_ref().unwrap().model, "a/m");
        assert_eq!(s.model("rumu/main").unwrap().efforts, main.efforts);
        assert!(
            s.set_roles(Roles {
                default: Some("rumu/nope".into()),
                title: None
            })
            .is_err()
        );
        s.set_roles(Roles {
            default: Some("rumu/main".into()),
            title: Some("rumu/main".into()),
        })
        .unwrap();

        // Renaming moves the roles along; a provider in use stays.
        let mut edit = custom("primary", "a/n");
        edit.rename = Some("main".into());
        let info = s.set_custom(edit).unwrap();
        assert_eq!(info.roles.default.as_deref(), Some("rumu/primary"));
        assert!(s.remove_provider("a").is_err());

        let again = Settings::load(home.path(), llm).unwrap();
        let file = again.lock().model.clone();
        assert_eq!(file.providers["a"].api_key, "k");
        assert_eq!(file.custom["primary"].model, "a/n");
        assert_eq!(file.roles.title.as_deref(), Some("rumu/primary"));

        s.remove_custom("primary").unwrap();
        assert_eq!(s.info().roles, Roles::default());
        // A provider a role names directly stays too.
        s.set_roles(Roles {
            default: Some("a/m".into()),
            title: None,
        })
        .unwrap();
        assert!(s.remove_provider("a").is_err());
        s.set_roles(Roles::default()).unwrap();
        s.remove_provider("a").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(home.path().join(SETTINGS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn offers_the_catalogs_providers_after_the_built_in_ones() {
        let home = tempfile::tempdir().unwrap();
        let s = Settings::load(home.path(), Arc::new(GenaiLlm::default())).unwrap();
        *s.catalog.write().unwrap() = Arc::new(
            Catalog::parse(
                br#"{
                  "deepseek": {"name":"DeepSeek","npm":"@ai-sdk/openai-compatible",
                    "api":"https://api.deepseek.com","models":{"m":{"tool_call":true}}},
                  "zeta": {"name":"Zeta","npm":"@ai-sdk/openai-compatible",
                    "api":"https://gw.example/v1","models":{"m":{"tool_call":true}}},
                  "alpha": {"name":"alpha","npm":"@ai-sdk/anthropic",
                    "api":"https://gw.example/anthropic/v1","models":{"m":{"tool_call":true}}},
                  "templated": {"npm":"@ai-sdk/openai-compatible",
                    "api":"https://${HOST}/v1","models":{"m":{"tool_call":true}}},
                  "no-tools": {"npm":"@ai-sdk/openai-compatible",
                    "api":"https://gw.example/v1","models":{"m":{}}}
                }"#,
            )
            .unwrap(),
        );

        // Built-in ones first (the catalog's copy of one is not repeated),
        // then the rest by name.
        let info = s.info();
        let n = presets::PRESETS.len();
        assert_eq!(info.presets[0].id, presets::PRESETS[0].id);
        let rest: Vec<_> = info.presets[n..].iter().map(|p| p.id.as_str()).collect();
        assert_eq!(rest, ["alpha", "zeta"]);
        assert_eq!(
            info.presets[n].endpoints[&Protocol::Anthropic],
            "https://gw.example/anthropic"
        );

        // Added, a catalog one keeps its endpoints whole and its name.
        let info = s
            .set_provider(ProviderEdit {
                id: "zeta".into(),
                name: None,
                preset: Some("zeta".into()),
                endpoints: info.presets[n + 1].endpoints.clone(),
                api_key: None,
                models: vec!["*".into()],
                protocols: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(info.providers[0].name, "Zeta");
        assert_eq!(
            s.lock().model.providers["zeta"].endpoints[&Protocol::Openai],
            "https://gw.example/v1"
        );
    }
}
