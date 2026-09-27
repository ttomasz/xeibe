//! Records the git commit for `xeibe --version` as `XEIBE_GIT_COMMIT`. Outside
//! a git checkout, or without `git`, the variable is left unset.

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let Some(commit) = git(&["rev-parse", "--short=12", "HEAD"]) else {
        return;
    };
    println!("cargo::rustc-env=XEIBE_GIT_COMMIT={commit}");

    // Rerun when HEAD moves: a new commit, a checkout, or a pack of the refs.
    // `--git-path` resolves each file correctly inside a worktree as well.
    let mut watched = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for file in watched {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &file]) {
            println!("cargo::rerun-if-changed={}", PathBuf::from(path).display());
        }
    }
}
