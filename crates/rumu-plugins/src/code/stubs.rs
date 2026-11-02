//! The deterministic API stub embedded in the `code` tool description.
//!
//! Hybrid mode keeps every tool in the model's tool list, so the stub repeats
//! only the *signatures*: one TypeScript-style line per tool (returning the
//! tool's output schema, or `string`) plus a one-line comment taken from the
//! tool description, marked `(parallel)` for concurrent tools. The same plugin set must produce
//! byte-identical text every startup (cache contract).
//!
//! `serde_json` is built with `preserve_order` (enabled explicitly in this
//! crate's manifest), so a schema's object properties are emitted in their
//! original order, exactly as the plugin declared them. Tool order is fixed by
//! `CodePlugin::new` (sorted by name).

use rumu_core::ToolSpec;
use serde_json::Value;

/// Max characters of the description used as the stub comment.
const COMMENT_LIMIT: usize = 120;

/// Render the whole stub block, tools already filtered and sorted by name.
pub(super) fn render(tools: &[ToolSpec]) -> String {
    let mut out = String::new();
    out.push_str(
        "/** Call any tool by name (names that are not JS identifiers go through here). */\n",
    );
    out.push_str("rumu.call(name: string, args?: Record<string, unknown>): Promise<string>\n");
    for tool in tools {
        out.push('\n');
        let parallel = if tool.parallel { " (parallel)" } else { "" };
        out.push_str(&format!(
            "/** {}{parallel} */\n",
            comment(&tool.description)
        ));
        out.push_str(&signature(tool));
        out.push('\n');
    }
    out
}

fn signature(tool: &ToolSpec) -> String {
    let args = ts_type(&tool.schema);
    let ret = tool
        .output_schema
        .as_ref()
        .map_or_else(|| "string".into(), ts_type);
    if is_js_ident(&tool.name) {
        format!("rumu.{}(args: {args}): Promise<{ret}>", tool.name)
    } else {
        // Quote the name: `rumu.weird-name` is not valid JS.
        format!(
            "rumu.call({}, args: {args}): Promise<{ret}>",
            serde_json::to_string(&tool.name).unwrap_or_else(|_| "\"?\"".into())
        )
    }
}

/// First sentence of a description, on one line, capped at [`COMMENT_LIMIT`].
fn comment(description: &str) -> String {
    let line = description.split('\n').next().unwrap_or_default().trim();
    let sentence = match line.find('.') {
        Some(i) => &line[..=i],
        None => line,
    }
    .trim();
    let mut s: String = sentence.chars().take(COMMENT_LIMIT).collect();
    if sentence.chars().count() > COMMENT_LIMIT {
        s.push('…');
    }
    // A description must not be able to close the block comment early.
    s.replace("*/", "* /")
}

fn is_js_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// JSON Schema → TypeScript type (best effort; `unknown` when unsure).
pub(super) fn ts_type(schema: &Value) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let parts: Vec<String> = values
            .iter()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "unknown".into()))
            .collect();
        if !parts.is_empty() {
            return parts.join(" | ");
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "string".into(),
        Some("number") | Some("integer") => "number".into(),
        Some("boolean") => "boolean".into(),
        Some("array") => {
            let item = schema.get("items").cloned().unwrap_or(Value::Null);
            format!("{}[]", ts_type(&item))
        }
        Some("object") => object_type(schema),
        _ => "unknown".into(),
    }
}

fn object_type(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return "Record<string, unknown>".into();
    };
    if props.is_empty() {
        return "Record<string, unknown>".into();
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let fields: Vec<String> = props
        .iter()
        .map(|(name, prop)| {
            let opt = if required.contains(&name.as_str()) {
                ""
            } else {
                "?"
            };
            format!("{name}{opt}: {}", ts_type(prop))
        })
        .collect();
    format!("{{ {} }}", fields.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(name: &str, description: &str, schema: Value) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            schema,
            background: false,
            output_schema: None,
            parallel: false,
        }
    }

    fn sample() -> Vec<ToolSpec> {
        vec![
            spec(
                "read",
                "Read a file's raw text content (no line numbers). `path` is relative.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "workspace": { "type": "string" },
                        "offset": { "type": "integer" },
                        "limit": { "type": "integer" }
                    },
                    "required": ["path"]
                }),
            ),
            spec(
                "weird-name",
                "A tool whose name is not a JS identifier.",
                json!({ "type": "object", "properties": {} }),
            ),
            spec(
                "tag",
                "Tag things. Two sentences.",
                json!({
                    "type": "object",
                    "properties": {
                        "mode": { "enum": ["a", "b"] },
                        "items": { "type": "array", "items": { "type": "boolean" } },
                        "meta": { "type": "object" }
                    }
                }),
            ),
        ]
    }

    #[test]
    fn snapshot() {
        let got = render(&sample());
        let want = "\
/** Call any tool by name (names that are not JS identifiers go through here). */
rumu.call(name: string, args?: Record<string, unknown>): Promise<string>

/** Read a file's raw text content (no line numbers). */
rumu.read(args: { path: string; workspace?: string; offset?: number; limit?: number }): Promise<string>

/** A tool whose name is not a JS identifier. */
rumu.call(\"weird-name\", args: Record<string, unknown>): Promise<string>

/** Tag things. */
rumu.tag(args: { mode?: \"a\" | \"b\"; items?: boolean[]; meta?: Record<string, unknown> }): Promise<string>
";
        assert_eq!(got, want);
    }

    #[test]
    fn deterministic_regardless_of_input_order() {
        let mut a = sample();
        let mut b = sample();
        b.reverse();
        a.sort_by(|x, y| x.name.cmp(&y.name));
        b.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(render(&a), render(&b));
    }

    #[test]
    fn comment_is_bounded_and_escaped() {
        assert_eq!(comment("One.\nTwo."), "One.");
        assert_eq!(comment("No period here"), "No period here");
        assert_eq!(comment("closes */ early."), "closes * / early.");
        let long = "x".repeat(200);
        let c = comment(&long);
        assert_eq!(c.chars().count(), COMMENT_LIMIT + 1);
        assert!(c.ends_with('…'));
    }

    #[test]
    fn identifier_check() {
        assert!(is_js_ident("read"));
        assert!(is_js_ident("$x_1"));
        assert!(!is_js_ident("weird-name"));
        assert!(!is_js_ident("1abc"));
    }
}
