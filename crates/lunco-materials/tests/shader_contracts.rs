//! Contracts that hold ACROSS shader files.
//!
//! `materials_test.rs` checks that one shader reflects the schema it declares.
//! These check the things that only break when shaders DISAGREE with each other,
//! or with the engine that loads them — the class no single-file test can see and
//! that a comment cannot enforce.
//!
//! Every assertion here corresponds to a defect that actually shipped:
//!
//! | contract | what it shipped as |
//! |---|---|
//! | `ORTHO_GAIN` on every albedo-map multiply | ground rendered at 41% albedo; a comment specified the gain "character-for-character" while the code did a plain multiply, and the web twin stayed wrong after the native was fixed |
//! | shared surface kernel, no local copies | `aa_fade` retuned in one file of six; the other four kept the old constants |
//! | full-arity `regolith_factor` | the opposition surge was dead code with zero call sites |
//! | every `lunco::` import has a keep-alive | terrain drew with NO material at all — flat grey, reported as "the ground went transparent" |
//! | photometry defaults agree | the same site would read differently depending on whether its terrain streamed |
//! | DEM normals cross one local-to-world boundary | coarse LODs became dark/black when a body-fixed site rotated relative to the render frame |
//!
//! Source-level on purpose: this crate deliberately carries no `wgpu`/`naga` (see
//! its Cargo.toml), and every defect above is visible in the text. A GPU-side
//! golden-image test is the complement, not the substitute — it catches "the
//! numbers changed", these catch "the files stopped agreeing".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn shaders_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(shaders_dir().join(name))
        .unwrap_or_else(|e| panic!("{name} readable: {e}"))
}

/// Every `.wgsl` under `assets/shaders`, as (file name, source).
fn all_shaders() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(shaders_dir()).expect("shaders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("wgsl") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        out.push((name, std::fs::read_to_string(&path).expect("read shader")));
    }
    out.sort();
    assert!(!out.is_empty(), "found no shaders to check");
    out
}

/// Strip `//` line comments so a contract cannot be satisfied by prose. Every
/// defect in the table above had a comment describing the correct behaviour; the
/// point of these tests is to read the CODE.
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────

/// A baked orthophoto is a 1–99 PERCENTILE STRETCH (mean ≈ 1/3), not a
/// reflectance map — so `albedo * map` DIMS the regolith by that mean instead of
/// tinting it. Measured on the shipped Apollo 15 ortho: mean 0.412, rendering the
/// authored 0.13 lunar albedo at 0.054.
///
/// Any shader that multiplies albedo by the authored map must apply `ORTHO_GAIN`.
#[test]
fn every_albedo_map_multiply_applies_ortho_gain() {
    for (name, src) in all_shaders() {
        let code = code_only(&src);
        if !code.contains("weight_albedo") {
            continue;
        }
        // The composite line: `mix(albedo, albedo * <map> ..., weight_albedo)`.
        let line = code
            .lines()
            .find(|l| l.contains("mix(albedo") && l.contains("weight_albedo"))
            .unwrap_or_else(|| {
                panic!("{name} declares weight_albedo but never composites it into albedo")
            });
        assert!(
            line.contains("ORTHO_GAIN"),
            "{name} multiplies albedo by the authored map WITHOUT ORTHO_GAIN — that renders \
             the site's real photograph as near-black mud (see lunar_brdf.wgsl). Line: {}",
            line.trim()
        );
        assert!(
            code.contains("ORTHO_GAIN")
                && src.contains("lunco::lunar::")
                && src.contains("ORTHO_GAIN"),
            "{name} must IMPORT ORTHO_GAIN from lunco::lunar, not redefine it — two copies \
             of that constant is how the native and web paths drifted apart"
        );
    }
}

/// The regolith surface kernel lives in `lunco::terrain` (terrain_surface.wgsl).
/// It was copy-pasted into six shaders; `aa_fade` was then retuned in exactly one
/// of them and the other five kept the old constants for weeks.
#[test]
fn terrain_shaders_import_the_surface_kernel_instead_of_copying_it() {
    const KERNEL_FNS: [&str; 4] = ["ramp", "aa_fade", "layer_height", "bump_layer"];
    for (name, src) in all_shaders() {
        if name == "terrain_surface.wgsl" {
            continue; // the kernel itself
        }
        let code = code_only(&src);
        for f in KERNEL_FNS {
            assert!(
                !code.contains(&format!("fn {f}(")),
                "{name} defines its own `{f}` — the surface kernel is shared via \
                 `#import lunco::terrain`. A local copy is how `aa_fade` ended up tuned \
                 in one file and stale in four others."
            );
        }
    }
}

/// The opposition surge — the retroreflective brightening toward zero phase angle
/// — is the defining lunar photometric effect. It existed as a function with ZERO
/// call sites while `regolith_factor` returned a view-independent factor.
///
/// Every consumer must call `regolith_factor` with the full parameter list, so a
/// shader cannot quietly fall back to a surge-less variant.
#[test]
fn every_regolith_factor_call_passes_the_photometry_params() {
    const PARAMS: [&str; 3] = ["surge_amp", "surge_width", "photometry_gain"];
    let mut consumers = 0;
    for (name, src) in all_shaders() {
        if name == "lunar_brdf.wgsl" {
            continue; // the definition
        }
        let code = code_only(&src);
        if !code.contains("regolith_factor(") {
            continue;
        }
        consumers += 1;
        // Calls span lines; check the whole call region rather than one line.
        let at = code.find("regolith_factor(").unwrap();
        let tail = &code[at..(at + 320).min(code.len())];
        for p in PARAMS {
            assert!(
                tail.contains(p),
                "{name} calls regolith_factor without `{p}` — every consumer must pass the \
                 full photometry set, or its surface silently uses different physics from \
                 the shader next to it. Call: {}",
                tail.lines().take(4).collect::<Vec<_>>().join(" ")
            );
        }
        for p in PARAMS {
            assert!(
                code.contains(&format!("{p}:")),
                "{name} passes `{p}` but does not declare it in its Material struct"
            );
        }
    }
    assert!(
        consumers >= 4,
        "expected at least 4 regolith_factor consumers, found {consumers} — if a terrain \
         shader stopped using the lunar BRDF, that is the thing to explain"
    );
}

/// The sun is a scene-global fact, picked STRUCTURALLY on the CPU and delivered as
/// `sun_dir_world`. `sun_to_light()` re-derived it in-shader by picking the
/// BRIGHTEST directional light — correct only while the sun outshines everything,
/// silent when it does not, and a different answer from the uniform the
/// static-mesh shaders were already using.
#[test]
fn no_shader_guesses_the_sun_from_light_brightness() {
    for (name, src) in all_shaders() {
        let code = code_only(&src);
        assert!(
            !code.contains("sun_to_light"),
            "{name} re-derives the sun in-shader. Declare `//!@engine sun_dir_world` and \
             read the uniform — see the note where `sun_to_light` used to live in \
             pbr_lit.wgsl."
        );
        assert!(
            !code.contains("directional_lights[0]"),
            "{name} indexes directional_lights[0] as the sun — the light array carries no \
             prim identity, so index 0 is not a sun. Use `sun_dir_world`."
        );
    }
}

/// A `#import lunco::foo` only resolves while something holds the module asset
/// alive. `ShaderMaterialPlugin` does that with a keep-alive `Resource` per
/// module. Miss one and the import fails, the WHOLE material fails to compose,
/// and the terrain renders with no shader at all — flat untextured grey, with the
/// real error buried in a `pipeline_cache` log line.
#[test]
fn every_imported_lunco_module_has_a_keepalive_in_the_plugin() {
    // module path (`lunco::terrain`) -> declaring file (`terrain_surface.wgsl`)
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    for (name, src) in all_shaders() {
        for line in src.lines() {
            let line = line.trim();
            if let Some(path) = line.strip_prefix("#define_import_path ") {
                declared.insert(path.trim().to_string(), name.clone());
            }
        }
    }

    let plugin = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../lunco-render-bevy/src/shader_material.rs"),
    )
    .expect("shader_material.rs readable");

    let mut checked = 0;
    for (name, src) in all_shaders() {
        for line in code_only(&src).lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("#import lunco::") else {
                continue;
            };
            // `lunco::terrain::{a, b}` / `lunco::lunar::regolith_factor` -> `terrain`
            let module = rest
                .split(&[':', '{', ' '][..])
                .next()
                .unwrap_or_default()
                .trim();
            if module.is_empty() {
                continue;
            }
            let full = format!("lunco::{module}");
            let file = declared.get(&full).unwrap_or_else(|| {
                panic!("{name} imports `{full}` but no shader declares that import path")
            });
            assert!(
                plugin.contains(&format!("shaders/{file}")),
                "`{full}` ({file}) is imported by {name} but has NO keep-alive resource in \
                 ShaderMaterialPlugin::build. Without one the asset is dropped, the import \
                 fails to resolve, and every material using it renders unshaded."
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "found no lunco:: imports to check — parser broken?"
    );
}

/// The streamed (`terrain_geomorph`) and static-mesh (`terrain_layered`,
/// `regolith`) paths shade the same authored site. If their photometry defaults
/// differ, the Moon looks different depending on whether the terrain streams —
/// which is exactly the divergence that let `ORTHO_GAIN` be right in one path and
/// missing in the other.
#[test]
fn photometry_defaults_agree_across_every_terrain_path() {
    const FILES: [&str; 4] = [
        "terrain_geomorph.wgsl",
        "terrain_layered.wgsl",
        "regolith.wgsl",
        "terrain_shadow.wgsl",
    ];
    const PARAMS: [&str; 3] = ["surge_amp", "surge_width", "photometry_gain"];

    let mut seen: BTreeMap<&str, (String, &str)> = BTreeMap::new();
    for file in FILES {
        let src = read(file);
        for p in PARAMS {
            let want = format!("//!@default {p}");
            let line = src
                .lines()
                .map(str::trim)
                .find(|l| l.starts_with(&want))
                .unwrap_or_else(|| panic!("{file} declares a default for `{p}`"));
            let value = line
                .split_whitespace()
                .nth(2)
                .unwrap_or_else(|| panic!("{file}: `{p}` default has no value"))
                .to_string();
            match seen.get(p) {
                None => {
                    seen.insert(p, (value, file));
                }
                Some((first, first_file)) => assert_eq!(
                    first, &value,
                    "`{p}` default disagrees: {first_file} says {first}, {file} says {value}. \
                     Both shade the same site; a divergence here is a divergence in how the \
                     Moon looks depending on whether the terrain streams."
                ),
            }
        }
    }
}

/// Derived terrain maps are baked in the DEM's local ENU coordinates. Both
/// render paths consume those same bytes while their mesh may be attached to a
/// rotating BigSpace frame, so neither path may decode the bytes directly into
/// a world-space lighting normal. The shared terrain kernel owns that one frame
/// crossing; this test prevents either material from quietly reintroducing a
/// second interpretation.
#[test]
fn every_derived_terrain_normal_uses_the_instance_frame_boundary() {
    let kernel = code_only(&read("terrain_surface.wgsl"));
    assert!(
        kernel.contains("fn dem_normal_to_world")
            && kernel.contains("mesh_functions::mesh_normal_local_to_world"),
        "the shared terrain kernel must transform DEM-local normals through the mesh instance"
    );

    for file in ["terrain_geomorph.wgsl", "terrain_layered.wgsl"] {
        let code = code_only(&read(file));
        assert!(
            code.contains("dem_normal_to_world("),
            "{file} must cross the shared DEM-local -> render-world normal boundary"
        );
        assert!(
            !code.contains("let n_baked = normalize("),
            "{file} decodes a derived normal directly as a lighting normal"
        );
    }
}

/// Procedural terrain detail must be registered to the DEM, not evaluated from
/// the transient BigSpace render position. The latter changes when the active
/// render origin is rebased and makes close terrain shimmer even when its
/// authored surface is unchanged.
#[test]
fn procedural_terrain_detail_is_anchored_to_dem_coordinates() {
    for file in [
        "terrain_geomorph.wgsl",
        "terrain_layered.wgsl",
        "regolith.wgsl",
    ] {
        let code = code_only(&read(file));
        assert!(
            code.contains("terrain_detail_position(in.uv")
                && code.contains("terrain_detail_normal_to_local")
                && code.contains("terrain_detail_normal_to_world"),
            "{file} must sample procedural detail in the DEM frame and transform its normal once"
        );
        assert!(
            !code.contains("surface_fbm(p ") && !code.contains("bump_layer(n, p"),
            "{file} still evaluates procedural detail from transient render-world position"
        );
    }
}

/// Physical terrain appearance must not encode CDLOD topology. Parent/child
/// substitution changes mesh depth, so feeding depth or the morph band into map
/// weights creates square AO/tone/normal changes even when geometry is seamless.
#[test]
fn streamed_terrain_map_weights_use_only_fragment_footprint() {
    let code = code_only(&read("terrain_geomorph.wgsl"));
    let kernel = code_only(&read("terrain_surface.wgsl"));
    assert!(
        code.contains("let map_footprint = pw / mat.map_texel_size_m;")
            && code.contains("let derived_weights = map_weights(map_footprint);")
            && code.contains("mat.derived_normal_on")
            && code.contains("mat.derived_surface_on"),
        "streamed terrain must derive engine-map detail from fragment footprint and explicit source contracts"
    );
    assert!(
        !code.contains("map_weights(mat.lod_depth")
            && !code.contains("map_weights(mat.morph_")
            && !code.contains("map_weights(mat.map_ratio"),
        "mesh depth or morph state leaked back into physical material appearance"
    );
    assert!(
        kernel.contains("return vec3(w_normal, 1.0, 1.0);"),
        "physical AO and tone must not change with camera distance"
    );
}

/// Terrain analysis is a tool material, not a production-shader branch. Keeping
/// its source separate means adding a diagnostic mode cannot add uniforms,
/// texture bindings, or divergent topology to every lunar terrain draw.
#[test]
fn terrain_diagnostic_material_is_separate_from_production_material() {
    let production = code_only(&read("terrain_geomorph.wgsl"));
    for forbidden in [
        "overlay_",
        "lod_depth",
        "weight_mineral",
        "mineral_tex",
        "slope_hazard_color",
    ] {
        assert!(
            !production.contains(forbidden),
            "production terrain material contains diagnostic contract `{forbidden}`"
        );
    }
    let diagnostic = code_only(&read("terrain_debug.wgsl"));
    assert!(diagnostic.contains("slope_hazard_color"));
    assert!(diagnostic.contains("lod_color"));
}

/// The published lunar fits these defaults came from (Chrono/UW-Madison,
/// arxiv 2410.04371 Table 1). Pinned so a future "tweak" is a deliberate,
/// reviewable edit rather than drift — the amplitude in particular was 0.8 for a
/// long time against a fitted 1.80238, and nothing said so.
#[test]
fn photometry_defaults_match_the_published_lunar_fit() {
    let src = read("terrain_geomorph.wgsl");
    for (param, expect) in [("surge_amp", 1.80_f32), ("surge_width", 0.0715_f32)] {
        let want = format!("//!@default {param}");
        let line = src
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(&want))
            .unwrap_or_else(|| panic!("default for `{param}`"));
        let got: f32 = line.split_whitespace().nth(2).unwrap().parse().unwrap();
        assert!(
            (got - expect).abs() < 1e-4,
            "`{param}` default is {got}, published lunar fit is {expect}. If this is a \
             deliberate departure, change the value AND this test together."
        );
    }
}
