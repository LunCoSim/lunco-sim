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
    let project_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon_rgba =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("luncosim-icon.rgba");
    write_window_icon(&project_dir, &icon_rgba);
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

fn write_window_icon(project_dir: &std::path::Path, icon_rgba: &std::path::Path) {
    let icon_name = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => "lcs-night-win.svg",
        Ok("macos") => "lcs-night-mac.svg",
        _ => "lcs-night-linux.svg",
    };
    let icon_svg = project_dir.join("../../assets/icons/svg").join(icon_name);
    println!("cargo:rerun-if-changed={}", icon_svg.display());

    let svg = std::fs::read(&icon_svg).unwrap_or_else(|error| {
        panic!(
            "failed to read LunCoSim window icon {}: {error}",
            icon_svg.display()
        )
    });
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).unwrap_or_else(|error| {
        panic!(
            "failed to parse LunCoSim window icon {}: {error}",
            icon_svg.display()
        )
    });
    let size = tree.size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(64, 64)
        .expect("64 by 64 LunCoSim window icon dimensions are valid");
    let transform =
        resvg::tiny_skia::Transform::from_scale(64.0 / size.width(), 64.0 / size.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    std::fs::write(icon_rgba, pixmap.data()).unwrap_or_else(|error| {
        panic!(
            "failed to write rendered LunCoSim window icon {}: {error}",
            icon_rgba.display()
        )
    });
}
