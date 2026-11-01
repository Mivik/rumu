//! The card library: `cards/<path>.md`-shaped storage under an
//! [`FsCardStore`] root, loaded recursively.
//!
//! Content is read from disk on demand (list / search / read / imports / the
//! update-notice hook) — the plugins keep no in-memory copy, so external
//! edits are picked up and can trigger update notices.

use std::collections::BTreeMap;

use crate::card::{Card, CardInfo, card_content, parse_card, under_prefix, valid_card_id};
use crate::store::FsCardStore;
use crate::{Error, Result};

/// Load every card (sorted by id) — used for listing, search, the catalog
/// and scope matching.
pub fn load_all(store: &FsCardStore) -> Result<BTreeMap<String, Card>> {
    let mut cards = BTreeMap::new();
    for id in store.walk()? {
        if let Some(card) = load_card(store, &id)? {
            cards.insert(id.clone(), card);
        }
    }
    Ok(cards)
}

/// Load one card by id. `Ok(None)` = no such card. The id must be valid (it
/// becomes a path).
pub fn load_card(store: &FsCardStore, id: &str) -> Result<Option<Card>> {
    if !valid_card_id(id) {
        return Ok(None);
    }
    Ok(store.read(id)?.map(|text| parse_card(id, &text)))
}

/// Write a card to the library, creating parent directories. Escape and
/// symlink guards are the store's job.
pub fn write_card(store: &FsCardStore, card: &Card) -> Result<()> {
    if !valid_card_id(&card.id) {
        return Err(Error::InvalidId {
            id: card.id.clone(),
        });
    }
    store.write(&card.id, &card_content(card))
}

/// All cards as [`CardInfo`] (serves the `card.list` WS method).
pub fn card_infos(store: &FsCardStore) -> Result<Vec<CardInfo>> {
    Ok(load_all(store)?
        .into_values()
        .map(|c| {
            let description = c.summary();
            let keywords = c.keywords().into_iter().map(str::to_owned).collect();
            CardInfo {
                id: c.id,
                description,
                keywords,
                body: c.body,
                revision: c.revision,
                meta: c.meta,
            }
        })
        .collect())
}

/// Cards under a directory prefix, as `(id, Card)` sorted by id.
pub fn cards_under(store: &FsCardStore, prefix: &str) -> Result<Vec<(String, Card)>> {
    Ok(load_all(store)?
        .into_iter()
        .filter(|(id, _)| under_prefix(id, prefix))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::card::parse_card;
    use crate::store::FsCardStore;

    #[test]
    fn recursive_load_skips_dot_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join("cards/dev/rumu")).unwrap();
        fs::write(home.join("cards/dev/rumu/kernel.md"), "# kernel\nnotes").unwrap();
        fs::write(home.join("cards/plain.md"), "plain").unwrap();
        fs::write(home.join("cards/dev/.hidden.md"), "hidden").unwrap();
        fs::write(home.join("cards/dev/notes.txt"), "not md").unwrap();
        let outside = dir.path().join("outside.md");
        fs::write(&outside, "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, home.join("cards/dev/escape.md")).unwrap();

        let store = FsCardStore::home(home);
        let ids = store.walk().unwrap();
        assert_eq!(ids, ["dev/rumu/kernel", "plain"]);

        let all = load_all(&store).unwrap();
        assert!(all.contains_key("dev/rumu/kernel"));
        assert!(all.contains_key("plain"));
        let kernel = &all["dev/rumu/kernel"];
        assert_eq!(kernel.body, "# kernel\nnotes"); // no frontmatter → whole text is body
        assert_eq!(kernel.render(), "# dev/rumu/kernel\n# kernel\nnotes");
    }

    #[test]
    fn write_creates_nested_dirs_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let store = FsCardStore::home(home);
        let card = parse_card("dev/rumu/new", "---\ntitle: New\n---\nbody\n");
        write_card(&store, &card).unwrap();
        assert!(home.join("cards/dev/rumu/new.md").is_file());
        let back = load_card(&store, "dev/rumu/new").unwrap().unwrap();
        assert_eq!(back.meta.get("title"), Some(&serde_json::json!("New")));
        assert_eq!(back.body, "body\n");

        // Ids are validated before they become paths.
        assert!(load_card(&store, "../evil").unwrap().is_none());
        assert!(load_card(&store, "").unwrap().is_none());
        assert!(write_card(&store, &parse_card("../evil", "x")).is_err());

        // A symlinked subdirectory inside cards/ must not be writable through.
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside");
            fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, home.join("cards/linked")).unwrap();
            let evil = parse_card("linked/evil", "owned");
            assert!(write_card(&store, &evil).is_err());
        }
    }

    #[test]
    fn card_infos_carry_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsCardStore::home(dir.path());
        let card = parse_card(
            "k",
            "---\ntitle: K\ndescription: A deployment guide\nkeywords: [deploy, rollout]\n\
             scope: [proj, 'proj:src/**']\npriority: 0\nactive: false\nextra: null\n\
             owner: {team: platform}\n---\nbody\n",
        );
        write_card(&store, &card).unwrap();
        let infos = card_infos(&store).unwrap();
        assert_eq!(infos[0].keywords, ["deploy", "rollout"]);
        assert_eq!(infos[0].description.as_deref(), Some("A deployment guide"));
        assert_eq!(infos[0].meta, card.meta);
        assert_eq!(infos[0].revision, card.revision);
        let value = serde_json::to_value(&infos[0]).unwrap();
        assert_eq!(
            value["meta"]["scope"],
            serde_json::json!(["proj", "proj:src/**"])
        );
        assert_eq!(value["meta"]["priority"], 0);
        assert_eq!(value["meta"]["active"], false);
        assert!(value["meta"]["extra"].is_null());
        assert_eq!(value["meta"]["owner"]["team"], "platform");
    }

    #[test]
    fn card_infos_omit_empty_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsCardStore::home(dir.path());
        write_card(&store, &parse_card("plain", "# Title\n- Summary\n")).unwrap();
        let infos = card_infos(&store).unwrap();
        assert_eq!(infos[0].description.as_deref(), Some("Summary"));
        assert!(infos[0].meta.is_empty());
        assert!(
            serde_json::to_value(&infos[0])
                .unwrap()
                .get("meta")
                .is_none()
        );
    }
}
