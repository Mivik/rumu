//! Session titles: the first prompt, truncated, right away; then, when a
//! profile is bound to the `title` role, a generated one in the background.

use std::sync::Arc;

use rumu_core::{Content, ModelPick, Node, NodeBody, NodeId, Role, SessionId, SessionInfo, now};
use serde_json::json;

use crate::Kernel;

/// Title length, in characters.
pub(crate) const TITLE_CHARS: usize = 60;

const TITLE_SYSTEM: &str = "You name chat sessions. Reply with only a short title \
(at most 8 words) for the conversation that starts with the user's message: \
no quotes, no trailing punctuation, in the language of the message.";

/// How much of the first prompt the title model sees.
const PROMPT_CHARS: usize = 2000;

impl Kernel {
    /// Generate a title for `session` from its first prompt, replacing the
    /// truncated one unless the title was changed meanwhile. Failures are
    /// only logged.
    pub(crate) fn spawn_title(self: &Arc<Self>, session: SessionInfo, prompt: String) {
        let k = self.clone();
        tokio::spawn(async move {
            let truncated = session.title.clone();
            let title = match k.generate_title(&session.id, &prompt).await {
                Ok(Some(t)) => t,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(session = %session.id, error = %e, "title generation failed");
                    return;
                }
            };
            let Ok(Some(mut current)) = k.store.get_session(&session.id) else {
                return;
            };
            if current.title != truncated {
                return;
            }
            current.title = title;
            if k.store.update_session(&current).is_ok() {
                k.push("session.updated", json!({ "session": current }));
            }
        });
    }

    async fn generate_title(
        &self,
        session: &SessionId,
        prompt: &str,
    ) -> crate::Result<Option<String>> {
        let node = Node {
            id: NodeId::new(),
            parent: None,
            session: session.clone(),
            at: now(),
            dur_ms: None,
            body: NodeBody::User {
                content: Content::text(prompt.chars().take(PROMPT_CHARS).collect::<String>()),
            },
        };
        let resp = self
            .llm
            .complete(
                ModelPick::Role(Role::Title),
                TITLE_SYSTEM,
                &[],
                &[node],
                None,
            )
            .await?;
        Ok(clean_title(&resp.text))
    }
}

/// The first non-empty line, unquoted, without trailing punctuation.
fn clean_title(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches('#').trim();
    let line = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '“' | '”' | '「' | '」'));
    let line = line.trim_end_matches(['.', '。', '!', '！']).trim();
    (!line.is_empty()).then(|| line.chars().take(TITLE_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::clean_title;

    #[test]
    fn cleans_titles() {
        assert_eq!(
            clean_title("\n\"Fix the build.\"\nmore").as_deref(),
            Some("Fix the build")
        );
        assert_eq!(clean_title("## 重构设置。").as_deref(), Some("重构设置"));
        assert_eq!(clean_title("  \n "), None);
    }
}
