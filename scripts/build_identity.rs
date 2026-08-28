// Shared build-identity stamping for application build scripts.

/// Stamp the release version and source revision into the current package.
pub(crate) fn stamp() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.is_empty());

    println!(
        "cargo:rustc-env=LUNCO_GIT_SHA={sha}{}",
        if dirty { "-dirty" } else { "" }
    );
    // Cargo's package version remains the source-of-truth base version. CI can
    // inject a SemVer2 product version without rewriting Cargo.toml.
    let release_version = std::env::var("LUNCO_RELEASE_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=LUNCO_RELEASE_VERSION={release_version}");
    println!("cargo:rerun-if-env-changed=LUNCO_RELEASE_VERSION");
    let repository = std::env::var("LUNCO_REPOSITORY_URL").unwrap_or_else(|_| {
        match (
            std::env::var("GITHUB_SERVER_URL"),
            std::env::var("GITHUB_REPOSITORY"),
        ) {
            (Ok(server), Ok(repository)) => format!("{server}/{repository}"),
            _ => "https://github.com/LunCoSim/lunco-sim".to_owned(),
        }
    });
    println!("cargo:rustc-env=LUNCO_REPOSITORY_URL={repository}");
    println!("cargo:rerun-if-env-changed=LUNCO_REPOSITORY_URL");
    println!("cargo:rerun-if-env-changed=GITHUB_SERVER_URL");
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");
    // Re-stamp when the checked-out revision moves. Resolve these through Git:
    // in a worktree, `../../.git/HEAD` is a gitfile rather than the real HEAD,
    // so watching that guessed path makes Cargo rebuild the package on every
    // invocation because the path does not exist. `HEAD` alone is not enough
    // on a branch: commits update `refs/heads/<branch>` while `HEAD` stays
    // unchanged. Packed refs cover repositories that have compacted that ref.
    for name in ["HEAD", "index", "packed-refs"] {
        emit_git_watch_path(name);
    }
    if let Some(reference) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
        if let Some(path) = git_metadata_path(&reference) {
            emit_existing_git_watch_path(&path);
        }
    }
    println!("cargo:rerun-if-changed=../../scripts/build_identity.rs");
}

fn emit_git_watch_path(name: &str) {
    if let Some(path) = git_metadata_path(name) {
        emit_existing_git_watch_path(&path);
    }
}

fn emit_existing_git_watch_path(path: &str) {
    if std::path::Path::new(path).exists() {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn git_metadata_path(name: &str) -> Option<String> {
    git_output(&["rev-parse", "--path-format=absolute", "--git-path", name])
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}
