use std::path::Path;
use std::sync::Arc;

use rumu_core::{PathState, Plugin, Utf8UnixPath};

use super::*;
use crate::testutil::FakeRuntime;

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

fn state(rules: &[&str]) -> PathState {
    PathState {
        skills: rules.iter().map(|r| r.to_string()).collect(),
        ..Default::default()
    }
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
        .enabled(&["*".into(), "!notes".into()])
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(enabled, ["pdf"]);
    assert!(skills.enabled(&[]).is_empty());
}

#[test]
fn lists_and_reads_the_enabled_skills() {
    let home = tempfile::tempdir().unwrap();
    let skills = fixture(home.path());
    let st = state(&["*", "!notes"]);
    let text = |out: ToolResult<ToolOutput>| out.unwrap().content.text;

    assert_eq!(
        text(list(&skills, &st, path("/skills"))),
        "pdf/ — Work with PDFs.\n"
    );
    assert_eq!(
        text(list(&skills, &st, path("/skills/pdf"))),
        "SKILL.md\nscripts/extract.py\n"
    );
    let read_skill = text(read(&skills, &st, path("/skills/pdf")));
    assert!(read_skill.starts_with("---\nname: pdf\n"), "{read_skill}");
    assert!(
        read_skill.ends_with(
            "Do the thing.\n\n---\nIts files, under `@rumu:/skills/pdf/`:\nSKILL.md\nscripts/extract.py"
        ),
        "{read_skill}"
    );
    assert_eq!(
        text(read(&skills, &st, path("/skills/pdf/scripts/extract.py"))),
        "print('text')\n"
    );

    let err = |out: ToolResult<ToolOutput>| out.unwrap_err().to_string();
    assert!(
        err(read(&skills, &st, path("/skills/notes"))).contains("no skill `notes` in this session")
    );
    assert!(err(read(&skills, &st, path("/skills/pdf/../../x"))).contains("leaves its skill"));
    assert!(err(read(&skills, &st, path("/skills/pdf/nope.md"))).contains("does not exist"));
    assert_eq!(
        text(list(&skills, &state(&[]), path("/skills"))),
        "This session has no skills."
    );
}

#[tokio::test]
async fn intro_note_lists_the_enabled_skills_once() {
    let home = tempfile::tempdir().unwrap();
    let plugin = SkillsPlugin::new(Arc::new(fixture(home.path())));
    let fake = FakeRuntime::arc();
    let event = rumu_core::Event {
        kind: "llm.pre".into(),
        session: Default::default(),
        branch: Default::default(),
        fields: Default::default(),
        payload: serde_json::Value::Null,
    };
    let none = plugin
        .on_hook(INTRO_HOOK, &event, &fake.cx())
        .await
        .unwrap();
    assert!(none.effects.is_empty());

    fake.state.lock().unwrap().skills = vec!["*".into()];
    let out = plugin
        .on_hook(INTRO_HOOK, &event, &fake.cx())
        .await
        .unwrap();
    let text = &out.effects[0].text;
    assert!(
        text.ends_with("\n- `pdf`: Work with PDFs.\n- `notes`: Take notes: when the user asks"),
        "{text}"
    );
}
