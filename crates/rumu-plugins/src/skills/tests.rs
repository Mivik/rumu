use std::path::Path;
use std::sync::Arc;

use rumu_core::{Plugin, Utf8UnixPath};

use super::*;
use crate::testutil::{FakeRuntime, assistant_decks, set_assistant};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDo the thing.\n")
}

/// `home` with an installed repository `kit` (skills `pdf` and a broken
/// one under `skills/`) and a host directory with another `pdf`, `notes`
/// (unquoted `: ` in its description) and `bare` (no frontmatter).
fn fixture(home: &Path) -> Skills {
    let kit = home.join("rumu/skills/kit/skills");
    write(
        &kit.join("pdf/SKILL.md"),
        &skill_md("pdf", "Work with PDFs."),
    );
    write(&kit.join("pdf/scripts/extract.py"), "print('text')\n");
    write(&kit.join("pdf/.git/HEAD"), "ignored\n");
    write(&kit.join("broken/SKILL.md"), "---\nname: broken\n---\n");
    let host = home.join(".agents/skills");
    write(
        &host.join("pdf/SKILL.md"),
        &skill_md("pdf", "Another PDF skill."),
    );
    write(
        &host.join("notes/SKILL.md"),
        &skill_md("notes", "Take notes: when the user asks"),
    );
    write(&host.join("bare/SKILL.md"), "# Bare\n");
    Skills::with_dirs(&home.join("rumu"), vec![host], Some(home.to_owned()))
}

fn state(rules: &[&str]) -> Vec<Vec<String>> {
    vec![rules.iter().map(|r| r.to_string()).collect()]
}

fn path(p: &str) -> &Utf8UnixPath {
    Utf8UnixPath::new(p)
}

#[test]
fn scans_in_precedence_order_leniently() {
    let home = tempfile::tempdir().unwrap();
    let skills = fixture(home.path());
    let info = skills.info();
    let rows: Vec<(&str, &str, Option<&str>, bool)> = info
        .skills
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.location.as_str(),
                s.repo.as_deref(),
                s.shadowed,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "broken",
                "~/rumu/skills/kit/skills/broken",
                Some("kit"),
                false
            ),
            ("pdf", "~/rumu/skills/kit/skills/pdf", Some("kit"), false),
            ("bare", "~/.agents/skills/bare", None, false),
            ("notes", "~/.agents/skills/notes", None, false),
            ("pdf", "~/.agents/skills/pdf", None, true),
        ]
    );
    let error = |name: &str| {
        info.skills
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.error.clone())
    };
    assert_eq!(
        error("broken").as_deref(),
        Some("SKILL.md has no description")
    );
    assert_eq!(
        error("bare").as_deref(),
        Some("SKILL.md has no frontmatter")
    );
    let notes = info.skills.iter().find(|s| s.name == "notes").unwrap();
    assert_eq!(notes.description, "Take notes: when the user asks");
    assert_eq!(info.repos.len(), 1);
    assert_eq!(info.repos[0].name, "kit");

    let enabled: Vec<String> = skills
        .enabled(&state(&["*", "!notes"]))
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(enabled, ["pdf"]);
    assert!(skills.enabled(&[]).is_empty());
    // The rule lists of two decks add up: one's exclusion does not take
    // away what the other enables.
    let both = [state(&["*", "!notes"]), state(&["notes"])].concat();
    let enabled: Vec<String> = skills.enabled(&both).into_iter().map(|s| s.name).collect();
    assert_eq!(enabled, ["pdf", "notes"]);
}

#[test]
fn lists_and_reads_the_enabled_skills() {
    let home = tempfile::tempdir().unwrap();
    let skills = fixture(home.path());
    let st = state(&["*", "!notes"]);
    let text = |out: ToolResult<ToolOutput>| out.unwrap().content.text;

    assert_eq!(
        list_skills(&skills, &st).content.text,
        "pdf/ — Work with PDFs.\n"
    );
    assert_eq!(
        text(list(&skills, &st, path("/skills/pdf"), "pdf", "")),
        "SKILL.md\nscripts/extract.py\n"
    );
    let read_skill = text(read(&skills, &st, path("/skills/pdf"), "pdf", ""));
    assert!(read_skill.starts_with("---\nname: pdf\n"), "{read_skill}");
    assert!(
        read_skill.ends_with(
            "Do the thing.\n\n---\nIts files, under `@rumu:/skills/pdf/`:\nSKILL.md\nscripts/extract.py"
        ),
        "{read_skill}"
    );
    let at = path("/skills/pdf/scripts/extract.py");
    assert_eq!(
        text(read(&skills, &st, at, "pdf", "scripts/extract.py")),
        "print('text')\n"
    );

    let err = |out: ToolResult<ToolOutput>| out.unwrap_err().to_string();
    assert!(
        err(read(&skills, &st, path("/skills/notes"), "notes", ""))
            .contains("no skill `notes` in this session")
    );
    let at = path("/skills/pdf/../../x");
    assert!(err(read(&skills, &st, at, "pdf", "../../x")).contains("leaves its skill"));
    let at = path("/skills/pdf/nope.md");
    assert!(err(read(&skills, &st, at, "pdf", "nope.md")).contains("does not exist"));
    assert_eq!(
        list_skills(&skills, &state(&[])).content.text,
        "This session has no skills."
    );
}

#[tokio::test]
async fn intro_note_follows_the_assistants_skills() {
    let home = tempfile::tempdir().unwrap();
    let decks = assistant_decks(home.path());
    let plugin = SkillsPlugin::new(Arc::new(fixture(home.path())), decks.clone());
    let fake = FakeRuntime::arc();
    let event = rumu_core::Event {
        kind: "llm.pre".into(),
        session: Default::default(),
        branch: Default::default(),
        fields: Default::default(),
        payload: serde_json::Value::Null,
    };
    let hook = || async {
        let out = plugin
            .on_hook(INTRO_HOOK, &event, &fake.cx())
            .await
            .unwrap();
        fake.apply_hook("skills", &out);
        out.effects.into_iter().map(|e| e.text).collect::<Vec<_>>()
    };
    assert!(hook().await.is_empty());

    set_assistant(&decks, Default::default(), &["*"]);
    let text = hook().await.remove(0);
    assert!(text.starts_with("Skills: "), "{text}");
    assert!(
        text.ends_with("\n- `pdf`: Work with PDFs.\n- `notes`: Take notes: when the user asks"),
        "{text}"
    );
    assert!(hook().await.is_empty());

    // Changing the assistant mid-session reaches the session.
    set_assistant(&decks, Default::default(), &["*", "!notes"]);
    let text = hook().await.remove(0);
    assert!(text.starts_with("The skills changed;"), "{text}");
    assert!(text.ends_with("\n- `pdf`: Work with PDFs."), "{text}");
    assert!(hook().await.is_empty());

    set_assistant(&decks, Default::default(), &[]);
    assert_eq!(hook().await, ["This session has no skills any more."]);
    assert!(hook().await.is_empty());
}

#[tokio::test]
async fn installs_updates_and_removes_a_repository() {
    let home = tempfile::tempdir().unwrap();
    let git = |dir: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .map(|o| o.status.success())
    };
    let origin = home.path().join("origin.git");
    write(&origin.join("SKILL.md"), &skill_md("solo", "One skill."));
    if !matches!(git(&origin, &["init", "-q"]), Ok(true)) {
        eprintln!("git not found; skipping");
        return;
    }
    assert!(git(&origin, &["add", "."]).unwrap());
    assert!(git(&origin, &["commit", "-qm", "init"]).unwrap());

    let skills = Skills::with_dirs(&home.path().join("rumu"), Vec::new(), None);
    let url = origin.display().to_string();
    assert_eq!(skills.install(&url, None).await.unwrap(), "origin");
    assert!(
        skills
            .install(&url, None)
            .await
            .unwrap_err()
            .contains("already installed")
    );
    let info = skills.info();
    assert_eq!(info.repos[0].url.as_deref(), Some(url.as_str()));
    assert_eq!(info.skills[0].name, "solo");

    write(
        &origin.join("SKILL.md"),
        &skill_md("solo", "Still one skill."),
    );
    assert!(git(&origin, &["commit", "-qam", "edit"]).unwrap());
    skills.update("origin").await.unwrap();
    assert_eq!(skills.info().skills[0].description, "Still one skill.");

    let bad = skills
        .install("/no/such/repo", Some("gone"))
        .await
        .unwrap_err();
    assert!(!bad.is_empty());
    assert!(!home.path().join("rumu/skills/gone").exists());

    skills.remove("origin").unwrap();
    assert!(skills.info().repos.is_empty());
    assert!(
        skills
            .remove("origin")
            .unwrap_err()
            .contains("no installed repository")
    );
}
