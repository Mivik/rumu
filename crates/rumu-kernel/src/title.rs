//! Generated session titles: with [`rumu_core::Titles::Model`], the first
//! prompt's truncated title is replaced in the background by one the `title`
//! role's model writes, and can be written anew on request.

use std::sync::Arc;

use rumu_core::{
    Content, ModelPick, Node, NodeBody, NodeId, Role, SessionId, SessionInfo, Titles, now,
};
use serde_json::json;

use crate::{Error, Kernel};

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
            if let Err(e) = k.write_title(&session, &prompt).await {
                tracing::warn!(session = %session.id, error = %e, "title generation failed");
            }
        });
    }

    /// Generate `id`'s title anew from the first prompt of its active
    /// branch; a session with no prompt yet keeps its title.
    pub async fn retitle(&self, id: &SessionId) -> crate::Result<()> {
        if self.llm.titles() != Titles::Model {
            return Err(Error::NoTitleModel);
        }
        let session = self
            .store
            .get_session(id)?
            .ok_or_else(|| Error::SessionNotFound(id.clone()))?;
        let (nodes, _) = self.branch_path(&session.main_branch)?;
        let prompt = nodes.iter().find_map(|n| match &n.body {
            NodeBody::User { content } if !content.text.trim().is_empty() => {
                Some(content.text.clone())
            }
            _ => None,
        });
        match prompt {
            Some(prompt) => self.write_title(&session, &prompt).await,
            None => Ok(()),
        }
    }

    /// Generate a title from `prompt` and set it on `session`, unless its
    /// title changed while the model wrote it.
    async fn write_title(&self, session: &SessionInfo, prompt: &str) -> crate::Result<()> {
        let Some(title) = self.generate_title(&session.id, prompt).await? else {
            return Ok(());
        };
        let Some(mut current) = self.store.get_session(&session.id)? else {
            return Ok(());
        };
        if current.title == session.title && current.title != title {
            current.title = title;
            self.store.update_session(&current)?;
            self.push("session.updated", json!({ "session": current }));
        }
        Ok(())
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
