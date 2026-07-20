//! Probe a scene's `terrain` variantSet: compose it once per variant and dump
//! the site-coupled values, so an authoring mistake (a site opinion left on the
//! base prim, where LIVRPS makes it stronger than every variant) shows up as
//! two variants reporting the same number.
//!
//! Usage: cargo run -p lunco-usd --example variant_probe -- <scene.usda> [set]

use lunco_usd::document::{LayerId, UsdDocument, UsdOp};
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_usd_bevy::{CanonicalStage, UsdRead};
use openusd::sdf::Path as SdfPath;

fn probe(label: &str, source: &str, tmp: &std::path::Path) {
    std::fs::write(tmp, source).expect("write temp scene");
    let stage = match lunco_usd_bevy::compose_file_to_stage(tmp) {
        Ok(s) => s,
        Err(e) => {
            println!("  {label}: COMPOSE FAILED: {e}");
            return;
        }
    };
    let cs = CanonicalStage::from_stage(stage, tmp.to_string_lossy().to_string());
    let view = cs.view();
    let p = |s: &str| SdfPath::new(s).unwrap();

    println!("\n── {label} ──────────────────────────────────────────────");
    println!(
        "  root anchor      lat {:?} lon {:?}",
        view.real(&p("/Traverse"), "lunco:anchor:lat"),
        view.real(&p("/Traverse"), "lunco:anchor:lon")
    );
    println!(
        "  sun              az {:?} el {:?}",
        view.real(&p("/Traverse"), "lunco:sun:azimuthDeg"),
        view.real(&p("/Traverse"), "lunco:sun:elevationDeg")
    );
    println!(
        "  demSource        {:?}",
        view.asset(&p("/Traverse/Terrain/Ground"), "lunco:layer:demSource")
    );
    // `targetRes` is an `int`, and `real()` reads only f64/f32 — reading it as a
    // real reports None for a perfectly well-composed attribute. The terrain
    // loader itself goes through `get_i64` (lunco-usd-terrain `attr_i32`), so
    // the probe must too or this check silently passes on a missing value.
    println!(
        "  windowM          {:?}  targetRes {:?}",
        view.real(&p("/Traverse/Terrain/Ground"), "lunco:layer:windowM"),
        view.attr_value(&p("/Traverse/Terrain/Ground"), "lunco:layer:targetRes")
    );
    let surf = p("/Traverse/Looks/TerrainLook/Surface");
    for role in ["albedo", "normal", "mineral"] {
        println!(
            "  {role:<8} map     {:?}  weight {:?}",
            view.asset(&surf, &format!("inputs:{role}_map")),
            view.real(&surf, &format!("inputs:weight_{role}"))
        );
    }
    for (name, path) in [
        ("POI", "/Traverse/POI"),
        ("Rover", "/Traverse/Rover"),
        ("Base", "/Traverse/Base"),
        ("Avatar", "/Traverse/Avatar"),
    ] {
        println!(
            "  {name:<8} xform    {:?}",
            view.value_vec3(&p(path), "xformOp:translate")
        );
    }
    // Route waypoints — the per-site trajectory the lessons read.
    let route = p("/Traverse/Route");
    let mut wps: Vec<String> = view
        .children(&route)
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    wps.sort();
    println!("  Route            {} waypoints", wps.len());
    for w in &wps {
        println!(
            "     {:<24} {:?}",
            w.rsplit('/').next().unwrap(),
            view.value_vec3(&p(w), "xformOp:translate")
        );
    }
    // Structural prims that must survive every variant.
    for must in [
        "/Traverse/Rover/Comms",
        "/Traverse/TeleopPolicy",
        "/Traverse/Terrain/Overzoom",
        "/Traverse/Terrain/Rocks",
    ] {
        if !view.has_prim(&p(must)) {
            println!("  ⚠ MISSING structural prim {must}");
        }
    }
}

/// Variant names authored in `set`, sorted and deduplicated.
///
/// Reads the layer directly rather than a composed stage — composition only
/// ever shows ONE selection at a time, which is exactly what this probe exists
/// to look past. Shares the scraper with the Inspector's variant picker so the
/// two can never disagree about what a scene offers.
fn discover_variants(src: &str, set: &str) -> Vec<String> {
    let Ok(data) = openusd::usda::parse(src) else {
        return Vec::new();
    };
    lunco_usd_bevy::variants::variant_options_in_data(&data)
        .remove(set)
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scene = args.get(1).expect("usage: variant_probe <scene.usda> [variantSet]");
    let set = args.get(2).map(String::as_str).unwrap_or("terrain");
    let src = std::fs::read_to_string(scene).expect("read scene");

    // Probe from INSIDE the engine's assets root: `shipped_asset_root` finds
    // the root by walking ancestors for a directory literally named `assets`,
    // so a `lunco://` reference in a file composed anywhere else (every twin)
    // cannot resolve.
    //
    // Mirror the scene's WHOLE directory, not just the one file: a scene may
    // reference a sibling by relative path (`@./traverse.usda@` — how a
    // variant-pinning wrapper works), and a lone temp copy would leave that
    // dangling. Relative texture paths (`terrain/…`) are attribute values, not
    // composition arcs, so they still do not need to resolve.
    let scene_dir = std::path::Path::new(scene).parent().unwrap();
    let probe_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets")
        .join(".variant_probe");
    let _ = std::fs::remove_dir_all(&probe_dir);
    std::fs::create_dir_all(&probe_dir).expect("probe dir");
    for entry in std::fs::read_dir(scene_dir).expect("read scene dir").flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "usda" || e == "usd") {
            let dest = probe_dir.join(p.file_name().unwrap());
            std::fs::copy(&p, &dest).expect("copy sibling scene");
        }
    }
    // The probed file keeps its own name so relative references to it resolve.
    let tmp = probe_dir.join(std::path::Path::new(scene).file_name().unwrap());

    // As authored (whatever selection the file carries).
    probe("as authored", &src, &tmp);

    // Discover the set's variants from the layer itself rather than hardcoding
    // them: a variant added to the scene and forgotten here would otherwise
    // never be probed, which is precisely the case worth catching.
    let variants = discover_variants(&src, set);
    if variants.is_empty() {
        println!("\n  no variants found in variantSet `{set}` — nothing to probe");
    }

    // Then force each variant through the same journaled op production uses.
    for variant in &variants {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(1),
            src.clone(),
            DocumentOrigin::writable_file(scene),
        );
        match doc.apply(UsdOp::SetVariantSelection {
            edit_target: LayerId::root(),
            path: "/Traverse".into(),
            variant_set: set.into(),
            variant: variant.into(),
        }) {
            Ok(_) => probe(&format!("{set} = {variant}"), &doc.source(), &tmp),
            Err(e) => println!("\n  {set}={variant}: SetVariantSelection failed: {e}"),
        }
    }
    let _ = std::fs::remove_dir_all(&probe_dir);
}
