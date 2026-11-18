//! Tool-argument schema generation and parsing, shared by the kernel's
//! built-in tools and the plugins.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{ToolError, ToolResult};

/// JSON Schema of a tool argument struct: a flat draft-07-shaped object with
/// inlined subschemas, no `$schema`/`title`/numeric-format noise, and no
/// `null` alternative for optional fields.
pub fn tool_schema<T: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft07().with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    });
    let mut value = settings
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value();
    strip_schema_noise(&mut value);
    value
}

/// Parse a tool's arguments into its `#[derive(Deserialize)]` struct. A
/// failure names the tool, e.g. ``read: invalid arguments: missing field
/// `path` ``.
pub fn parse_args<T: DeserializeOwned>(tool: &str, args: &Value) -> ToolResult<T> {
    serde_json::from_value(args.clone())
        .map_err(|e| ToolError::model(format!("{tool}: invalid arguments: {e}")))
}

/// Remove schema machinery the model does not need: titles, numeric
/// formats/bounds, and the `null` branch schemars adds for `Option<T>`.
fn strip_schema_noise(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in ["title", "format", "minimum", "maximum", "default"] {
                map.remove(key);
            }
            // `Option<Enum>` also adds a null enum member.
            if let Some(Value::Array(values)) = map.get_mut("enum") {
                values.retain(|v| !v.is_null());
            }
            // Keep "null" only when it is the sole type (an actual null schema).
            let mut sole_type = None;
            if let Some(Value::Array(types)) = map.get_mut("type")
                && types.len() > 1
            {
                types.retain(|t| t.as_str() != Some("null"));
                if types.len() == 1 {
                    sole_type = Some(types.remove(0));
                }
            }
            if let Some(only) = sole_type {
                map.insert("type".into(), only);
            }
            for child in map.values_mut() {
                strip_schema_noise(child);
            }
            // Option<...> wraps enum/object schemas in `anyOf [T, null]`; drop
            // the null branch and flatten the redundant single-branch wrapper.
            for key in ["anyOf", "oneOf"] {
                let Some(Value::Array(mut branches)) = map.remove(key) else {
                    continue;
                };
                branches.retain(|b| !is_null_schema(b));
                match branches.len() {
                    0 => {}
                    1 => match branches.pop() {
                        Some(Value::Object(inner)) => map.extend(inner),
                        other => {
                            map.insert(key.into(), Value::Array(other.into_iter().collect()));
                        }
                    },
                    _ => {
                        map.insert(key.into(), Value::Array(branches));
                    }
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_schema_noise),
        _ => {}
    }
}

fn is_null_schema(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|obj| obj.len() == 1 && obj.get("type").is_some_and(|t| t == "null"))
}
