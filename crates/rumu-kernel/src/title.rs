//! Session titles: the first prompt cut short, or — with
//! [`rumu_core::Titles::Model`] — one the `title` role's model writes in the
//! background. The model can be asked again later, and a title can be written
//! by hand.

use std::sync::Arc;

use rumu_core::{
    Content, ModelPick, Node, NodeBody, NodeId, Role, SessionId, SessionInfo, Titles, now,
};
use serde_json::json;

use crate::{Error, Kernel};

/// Title length, in characters.
pub(crate) const TITLE_CHARS: usize = 60;

const TITLE_SYSTEM: &str = include_str!("../prompts/title-system.md");

/// How much of the first prompt, the first reply and the latest prompt the
/// title model sees.
const FIRST_PROMPT_CHARS: usize = 2000;
const FIRST_REPLY_CHARS: usize = 1000;
const LATEST_PROMPT_CHARS: usize = 500;

impl Kernel {
    /// Generate a title for `session` in the background, replacing the
    /// truncated one unless the title was changed meanwhile. Failures are
    /// only logged.
    pub(crate) fn spawn_title(self: &Arc<Self>, session: SessionInfo) {
        let k = self.clone();
        tokio::spawn(async move {
            if let Err(e) = k.write_title(&session).await {
                tracing::warn!(session = %session.id, error = %e, "title generation failed");
            }
        });
    }

    /// Generate `id`'s title anew from its main branch as it stands now; a
    /// session with no prompt yet keeps its title.
    pub async fn retitle(&self, id: &SessionId) -> crate::Result<()> {
        if self.llm.titles() != Titles::Model {
            return Err(Error::NoTitleModel);
        }
        let session = self
            .store
            .get_session(id)?
            .ok_or_else(|| Error::SessionNotFound(id.clone()))?;
        self.write_title(&session).await
    }

    /// Set `id`'s title by hand, on one line and cut to [`TITLE_CHARS`]. A
    /// blank title leaves it alone.
    pub fn set_title(&self, id: &SessionId, title: &str) -> crate::Result<()> {
        let title = one_line(title);
        let mut session = self
            .store
            .get_session(id)?
            .ok_or_else(|| Error::SessionNotFound(id.clone()))?;
        if title.is_empty() || title == session.title {
            return Ok(());
        }
        session.title = title;
        self.store.update_session(&session)?;
        self.push("session.updated", json!({ "session": session }));
        Ok(())
    }

    /// Generate a title and set it on `session`, unless its title changed
    /// while the model wrote it.
    async fn write_title(&self, session: &SessionInfo) -> crate::Result<()> {
        let Some(context) = self.title_context(session)? else {
            return Ok(());
        };
        let Some(title) = self.generate_title(&session.id, context).await? else {
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

    /// What the title model reads: the first prompt, the first reply with
    /// text, and the latest prompt, each cut short. `None` before the first
    /// prompt.
    fn title_context(&self, session: &SessionInfo) -> crate::Result<Option<String>> {
        let (nodes, _) = self.branch_path(&session.main_branch)?;
        let mut prompts = nodes.iter().filter_map(|n| match &n.body {
            NodeBody::User { content } if !content.text.trim().is_empty() => {
                Some(content.text.as_str())
            }
            _ => None,
        });
        let Some(first) = prompts.next() else {
            return Ok(None);
        };
        let latest = prompts.next_back();
        let reply = nodes.iter().find_map(|n| match &n.body {
            NodeBody::Assistant { content, .. } if !content.text.trim().is_empty() => {
                Some(content.text.as_str())
            }
            _ => None,
        });

        let mut text = format!(
            "The conversation starts with:\n{}",
            cut(first, FIRST_PROMPT_CHARS)
        );
        if let Some(reply) = reply {
            text += "\n\nThe first reply starts with:\n";
            text += &cut(reply, FIRST_REPLY_CHARS);
        }
        if let Some(latest) = latest {
            text += "\n\nThe user's latest message:\n";
            text += &cut(latest, LATEST_PROMPT_CHARS);
        }
        Ok(Some(text))
    }

    async fn generate_title(
        &self,
        session: &SessionId,
        context: String,
    ) -> crate::Result<Option<String>> {
        let node = Node {
            id: NodeId::new(),
            parent: None,
            session: session.clone(),
            at: now(),
            dur_ms: None,
            body: NodeBody::User {
                content: Content::text(context),
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

/// `text` cut to `n` characters.
fn cut(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `text` with its whitespace runs collapsed to single spaces, cut to
/// [`TITLE_CHARS`].
fn one_line(text: &str) -> String {
    cut(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        TITLE_CHARS,
    )
}

/// The first non-empty line, without markdown, quotes, a `Title:` label or
/// trailing punctuation.
fn clean_title(text: &str) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let line = unwrap(line);
    let line = ["Title", "title", "TITLE", "标题"]
        .into_iter()
        .find_map(|label| {
            line.strip_prefix(label)?
                .trim_start()
                .strip_prefix([':', '：'])
        })
        .map(unwrap)
        .filter(|rest| !rest.is_empty())
        .unwrap_or(line);
    let line = unwrap(line.trim_end_matches(['.', '。', '!', '！']));
    (!line.is_empty()).then(|| cut(line, TITLE_CHARS))
}

/// `line` without a leading `#` heading mark, or the bold and quotes around it.
fn unwrap(line: &str) -> &str {
    line.trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches(['*', '"', '\'', '`', '“', '”', '「', '」', '『', '』'])
        .trim()
}

#[cfg(test)]
mod tests {
    use super::{clean_title, one_line};

    #[test]
    fn cleans_titles() {
        let cases = [
            ("\n\"Fix the build.\"\nmore", "Fix the build"),
            ("## 重构设置。", "重构设置"),
            ("Learn C#", "Learn C#"),
            ("Title: Fix the build", "Fix the build"),
            ("**Title:** Fix the build", "Fix the build"),
            ("标题：「重构设置页」", "重构设置页"),
            // only a label is dropped, not the word
            ("标题栏高度", "标题栏高度"),
        ];
        for (raw, title) in cases {
            assert_eq!(clean_title(raw).as_deref(), Some(title), "{raw:?}");
        }
        assert_eq!(clean_title("  \n "), None);
    }

    #[test]
    fn hand_written_titles_go_on_one_line() {
        assert_eq!(
            one_line("  Fix the\n  slider tooltip "),
            "Fix the slider tooltip"
        );
        assert_eq!(one_line(&"字".repeat(80)).chars().count(), 60);
    }
}
