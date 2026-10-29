//! YAML parsing for config files and card frontmatter.
//!
//! `serde-saphyr` infers YAML 1.1 boolean forms (`y`, `yes`, `on`, `off`, …)
//! when the target type is a generic value; our files follow the YAML 1.2
//! spelling, so `reason: y` stays the string `"y"` rather than becoming
//! `true`.

use serde::de::DeserializeOwned;

/// Parse one YAML document with the project's options.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, serde_saphyr::Error> {
    serde_saphyr::from_str_with_options(text, serde_saphyr::options! { strict_booleans: true })
}
