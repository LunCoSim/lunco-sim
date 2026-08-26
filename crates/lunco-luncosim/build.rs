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

use std::path::Path;

mod build_identity {
    include!("../../scripts/build_identity.rs");
}

fn main() {
    let project_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let icon_svg = project_dir
        .join("../../assets/icons/svg")
        .join(platform_icon_name(&target_os));
    println!("cargo:rerun-if-changed={}", icon_svg.display());
    println!("cargo:rerun-if-env-changed=LUNCOSIM_ICON_OUTPUT_DIR");
    println!("cargo:rerun-if-env-changed=LUNCOSIM_ICON_OUTPUT_STAMP");

    let svg = std::fs::read(&icon_svg).unwrap_or_else(|error| {
        panic!(
            "failed to read LunCoSim icon source {}: {error}",
            icon_svg.display()
        )
    });
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).unwrap_or_else(|error| {
        panic!(
            "failed to parse LunCoSim icon source {}: {error}",
            icon_svg.display()
        )
    });

    let icon_rgba = out_dir.join("luncosim-icon.rgba");
    write_window_icon(&tree, &icon_rgba);
    if target_os == "windows" {
        let icon_ico = out_dir.join("luncosim.ico");
        write_windows_ico(&tree, &icon_ico);
        embed_windows_icon(&icon_ico);
    }
    if let Some(icon_output_dir) = std::env::var_os("LUNCOSIM_ICON_OUTPUT_DIR") {
        write_package_icons(&tree, &target_os, Path::new(&icon_output_dir));
    }
    build_identity::stamp();
}

fn platform_icon_name(target_os: &str) -> &'static str {
    match target_os {
        "windows" => "lcs-night-win.svg",
        "macos" => "lcs-night-mac.svg",
        _ => "lcs-night-linux.svg",
    }
}

fn write_window_icon(tree: &usvg::Tree, icon_rgba: &Path) {
    let pixmap = render_icon(tree, 64);
    std::fs::write(icon_rgba, pixmap.data()).unwrap_or_else(|error| {
        panic!(
            "failed to write rendered LunCoSim window icon {}: {error}",
            icon_rgba.display()
        )
    });
}

fn render_icon(tree: &usvg::Tree, size: u32) -> resvg::tiny_skia::Pixmap {
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(size, size).expect("LunCoSim icon dimensions are valid");
    let source = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(
        size as f32 / source.width(),
        size as f32 / source.height(),
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap
}

fn write_windows_ico(tree: &usvg::Tree, destination: &Path) {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::ExtendedColorType;

    let sizes = [16_u32, 24, 32, 48, 64, 128, 256];
    let frames: Vec<IcoFrame<'static>> = sizes
        .into_iter()
        .map(|size| {
            let pixmap = render_icon(tree, size);
            IcoFrame::as_png(pixmap.data(), size, size, ExtendedColorType::Rgba8)
                .unwrap_or_else(|error| panic!("failed to encode {size}px LunCoSim icon: {error}"))
        })
        .collect();
    let mut file = std::fs::File::create(destination).unwrap_or_else(|error| {
        panic!(
            "failed to write LunCoSim executable icon {}: {error}",
            destination.display()
        )
    });
    IcoEncoder::new(&mut file)
        .encode_images(&frames)
        .unwrap_or_else(|error| panic!("failed to encode LunCoSim executable icon: {error}"));
}

fn write_package_icons(tree: &usvg::Tree, target_os: &str, output_dir: &Path) {
    std::fs::create_dir_all(output_dir).unwrap_or_else(|error| {
        panic!(
            "failed to create LunCoSim package icon directory {}: {error}",
            output_dir.display()
        )
    });

    match target_os {
        "windows" => write_windows_ico(tree, &output_dir.join("luncosim.ico")),
        "macos" => write_macos_iconset(tree, &output_dir.join("macos/luncosim.iconset")),
        _ => write_linux_icons(tree, &output_dir.join("linux")),
    }
}

fn write_macos_iconset(tree: &usvg::Tree, iconset_dir: &Path) {
    for size in [16_u32, 32, 128, 256, 512] {
        write_png(
            tree,
            size,
            &iconset_dir.join(format!("icon_{size}x{size}.png")),
        );
        write_png(
            tree,
            size * 2,
            &iconset_dir.join(format!("icon_{size}x{size}@2x.png")),
        );
    }
}

fn write_linux_icons(tree: &usvg::Tree, linux_dir: &Path) {
    let sizes = [16_u32, 24, 32, 48, 64, 128, 256];
    for size in sizes {
        let pixmap = render_icon(tree, size);
        let png = pixmap
            .encode_png()
            .unwrap_or_else(|error| panic!("failed to encode {size}px LunCoSim PNG: {error}"));
        let destination = linux_dir
            .join("hicolor")
            .join(format!("{size}x{size}"))
            .join("apps/luncosim.png");
        write_bytes(&png, &destination);
        if size == 256 {
            write_bytes(&png, &linux_dir.join("luncosim.png"));
        }
    }
}

fn write_png(tree: &usvg::Tree, size: u32, destination: &Path) {
    let pixmap = render_icon(tree, size);
    let png = pixmap
        .encode_png()
        .unwrap_or_else(|error| panic!("failed to encode {size}px LunCoSim PNG: {error}"));
    write_bytes(&png, destination);
}

fn write_bytes(bytes: &[u8], destination: &Path) {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!(
                "failed to create LunCoSim icon directory {}: {error}",
                parent.display()
            )
        });
    }
    std::fs::write(destination, bytes).unwrap_or_else(|error| {
        panic!(
            "failed to write LunCoSim package icon {}: {error}",
            destination.display()
        )
    });
}

#[cfg(windows)]
fn embed_windows_icon(icon: &std::path::Path) {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon.to_str().expect("Windows icon path is valid UTF-8"));
    resource
        .compile()
        .expect("failed to embed LunCoSim executable icon");
}

#[cfg(not(windows))]
fn embed_windows_icon(_icon: &std::path::Path) {}
