//! The protocol-v1 half of the ACP translation: [`Event`]s rendered as
//! `session/update`s, and prompts read into rumu text.

use agent_client_protocol::v1::schema::{
    ContentBlock, ContentChunk, EmbeddedResourceResource, SessionUpdate,
};

use super::events::Event;

/// The `session/update` an event is.
pub(super) fn update(event: Event) -> SessionUpdate {
    match event {
        Event::Text(text) => SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into())),
        Event::Thought(text) => SessionUpdate::AgentThoughtChunk(ContentChunk::new(text.into())),
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
