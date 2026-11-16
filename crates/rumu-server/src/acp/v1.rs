//! The protocol-v1 half of the ACP translation: [`Event`]s rendered as
//! `session/update`s, and prompts read into rumu text.

use agent_client_protocol::v1::schema::{
    ContentBlock, ContentChunk, Diff, EmbeddedResourceResource, Meta, SessionUpdate, ToolCall,
    ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};
use serde_json::json;

use super::events::{Event, Kind};

/// The `session/update` an event is.
pub(super) fn update(event: Event) -> SessionUpdate {
    match event {
        Event::Text(text) => SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into())),
        Event::Thought(text) => SessionUpdate::AgentThoughtChunk(ContentChunk::new(text.into())),
        Event::ToolStart {
            id,
            name,
            title,
            kind,
            input,
            locations,
        } => SessionUpdate::ToolCall(
            ToolCall::new(id, title)
                .name(name)
                .kind(tool_kind(kind))
                .status(ToolCallStatus::InProgress)
                .raw_input(input)
                .locations(locations.into_iter().map(ToolCallLocation::new).collect()),
        ),
        Event::ToolOutput { id, text } => SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            id,
            ToolCallUpdateFields::new().content(vec![text_content(text)]),
        )),
        Event::ToolEnd {
            id,
            failed,
            text,
            diffs,
        } => {
            let mut content = vec![];
            if !text.is_empty() {
                content.push(text_content(text));
            }
            content.extend(
                diffs
                    .into_iter()
                    .map(|d| ToolCallContent::Diff(Diff::new(d.path, d.new).old_text(d.old))),
            );
            let status = if failed {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            };
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                id,
                ToolCallUpdateFields::new().status(status).content(content),
            ))
        }
        Event::ToolChild { id, branch } => {
            let meta = Meta::from_iter([("rumu".to_owned(), json!({ "branch": branch }))]);
            SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(id, ToolCallUpdateFields::new()).meta(meta),
            )
        }
    }
}

fn text_content(text: String) -> ToolCallContent {
    ToolCallContent::Content(agent_client_protocol::v1::schema::Content::new(
        ContentBlock::from(text),
    ))
}

fn tool_kind(kind: Kind) -> ToolKind {
    match kind {
        Kind::Read => ToolKind::Read,
        Kind::Edit => ToolKind::Edit,
        Kind::Execute => ToolKind::Execute,
        Kind::Search => ToolKind::Search,
        Kind::Other => ToolKind::Other,
    }
}

/// A prompt as rumu text: text as is, a linked resource as a markdown link,
/// an embedded text resource as a `<context>` block. Media is not offered in
/// the prompt capabilities, so a client sending it anyway loses it.
pub(super) fn prompt_text(blocks: &[ContentBlock]) -> String {
    let mut parts = vec![];
    for block in blocks {
        match block {
            ContentBlock::Text(t) => parts.push(t.text.clone()),
            ContentBlock::ResourceLink(link) => {
                parts.push(format!("[{}]({})", link.name, link.uri))
            }
            ContentBlock::Resource(res) => {
                if let EmbeddedResourceResource::TextResourceContents(c) = &res.resource {
                    parts.push(format!(
                        "<context uri=\"{}\">\n{}\n</context>",
                        c.uri, c.text
                    ));
                }
            }
            _ => {}
        }
    }
    parts.join("\n\n")
}
