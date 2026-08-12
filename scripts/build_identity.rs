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
    // Re-stamp when the checked-out revision moves. Without this, the build
    // can report the revision from an earlier checkout.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../scripts/build_identity.rs");
}
