//! `RUMU_VERSION`: the package version and, in a git checkout, the commit it
//! was built from (`0.1.0 (dbd7d1c 2026-11-25)`).

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    (out.status.success() && !s.trim().is_empty()).then(|| s.trim().to_owned())
}

fn main() {
    let pkg = env!("CARGO_PKG_VERSION");
    let version = match git(&["log", "-1", "--format=%h %cs"]) {
        Some(commit) => format!("{pkg} ({commit})"),
        None => pkg.to_owned(),
    };
    println!("cargo:rustc-env=RUMU_VERSION={version}");

    // Rerun when HEAD moves: a checkout, or a commit on the current branch.
    println!("cargo:rerun-if-changed=build.rs");
    let mut watch = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    watch.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
