//! Stamp the build's git revision into the binary.
//!
//! A tester's log has to identify the build that produced it. Five Windows tester
//! runs in the 2026-07-26 report could only be told apart by install path and asset
//! counts, which is why that report has a "build grouping" paragraph instead of a
//! build column.
//!
//! Failure is not fatal: a source tarball or a vendored build has no git, and a
//! simulator that refuses to compile outside a checkout would be worse than one whose
//! log says `unknown`.

fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| !o.stdout.is_empty());

    println!(
        "cargo:rustc-env=LUNCO_GIT_SHA={sha}{}",
        if dirty { "-dirty" } else { "" }
    );
    // Re-stamp when the checked-out revision moves. Without this the sha is baked at
    // first compile and every later build lies about which commit it is.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
}
