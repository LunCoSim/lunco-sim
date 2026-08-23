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

mod build_identity {
    include!("../../scripts/build_identity.rs");
}

fn main() {
    let project_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let icon_rgba = out_dir.join("luncosim-icon.rgba");
    write_window_icon(&project_dir, &icon_rgba);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon_ico = out_dir.join("luncosim.ico");
        write_windows_ico(&project_dir, &icon_ico);
        embed_windows_icon(&icon_ico);
    }
    build_identity::stamp();
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
    let pixmap = render_icon(&tree, 64);
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

fn write_windows_ico(project_dir: &std::path::Path, destination: &std::path::Path) {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::ExtendedColorType;

    let icon_svg = project_dir
        .join("../../assets/icons/svg")
        .join("lcs-night-win.svg");
    let svg = std::fs::read(&icon_svg).unwrap_or_else(|error| {
        panic!(
            "failed to read LunCoSim executable icon {}: {error}",
            icon_svg.display()
        )
    });
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).unwrap_or_else(|error| {
        panic!(
            "failed to parse LunCoSim executable icon {}: {error}",
            icon_svg.display()
        )
    });

    let sizes = [16_u32, 24, 32, 48, 64, 128, 256];
    let frames: Vec<IcoFrame<'static>> = sizes
        .into_iter()
        .map(|size| {
            let pixmap = render_icon(&tree, size);
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
