//! How free text appears inside a view rendered for the model.

/// Characters of free text a one-line entry (a task list line, a label in a
/// note) shows.
pub const QUOTE_CHARS: usize = 80;

/// Free text (a sub-agent's task, a title, a command, a snippet) as it
/// appears inside a rendered view: a JSON string literal of its first `max`
/// chars, so its newlines and quotes cannot break the view's layout. A cut
/// appends `…(+N chars)` after the closing quote, so the literal stays the
/// text's exact prefix and says how much is missing.
///
/// Every model-facing view embeds free text through this, instead of each
/// cutting and escaping its own way.
pub fn quote(text: &str, max: usize) -> String {
    let total = text.chars().count();
    let cut: String = text.chars().take(max).collect();
    let mut q = serde_json::to_string(&cut).unwrap_or_default();
    if total > max {
        q.push_str(&format!("…(+{} chars)", total - max));
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_escape_and_cut_outside_the_literal() {
        assert_eq!(quote("a \"b\"\nc", 10), r#""a \"b\"\nc""#);
        assert_eq!(quote("什么都可以", 3), "\"什么都\"…(+2 chars)");
        assert_eq!(quote("abc", 3), "\"abc\"");
        assert_eq!(quote("", 3), "\"\"");
    }
}
