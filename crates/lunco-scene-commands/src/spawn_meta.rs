// The ONE parser for the spawn metadata a `*.usda` authors on its default prim.
// Shared verbatim by the library and by `build.rs`, which `include!`s this file
// (a build script cannot depend on the crate it builds). Inner doc comments
// (`//!`) are therefore illegal here — an `include!`d file is spliced mid-module.
//
// WHY THIS FILE EXISTS. There were three copies of this logic:
// `catalog::read_spawn_meta` (native), `build.rs::parse_meta` (the wasm bake), and
// `catalog::read_usd_description` (native, via a *real* USD parse). The first two
// carried a comment saying "keep all three in sync" — a comment that only gets
// written because they already drifted, and they had: the description was read one
// way natively and baked another way for the web, so the two platforms could
// legitimately disagree about what a file said about itself.
//
// SCOPE. A deliberate line scan, not a USD parse: `build.rs` runs before the crate
// (and its USD stack) exists. The three attributes are simple scalars on the
// default prim, so a scan suffices — but a **multi-line** string value is not
// supported, which the tests pin rather than leave to be discovered.
//
// TODO(bake): the web reads these from a table `build.rs` compiles into the
// binary, purely because the browser has no filesystem. But we *ship* the assets —
// the web should read them over HTTP through `lunco-storage` like everything else,
// and the bake should not exist. That needs an asset-backed `Storage` impl (today:
// FileStorage / WebStorage(localStorage) / OPFS only) and an async catalogue scan.

/// Spawn metadata authored on a `*.usda`'s default prim.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnMeta {
    /// `bool lunco:spawnable` — whether the file is a spawnable part.
    ///
    /// **Opt-in.** Default `false`: a file is offered in the palette only if it
    /// says it is a part. This used to default to `true`, which meant the
    /// catalogue offered *every* USD asset in the project and every scene, mission
    /// and scenario had to remember to disclaim it — and three scenes had silently
    /// forgotten to, so they showed up as spawnable parts. A default that must be
    /// disclaimed everywhere is a default that leaks.
    pub spawnable: bool,
    /// `float lunco:spawnLift` — metres to lift the spawn point.
    pub lift: f32,
    /// `string lunco:description` — the blurb shown as a palette/Scenarios tooltip.
    pub description: Option<String>,
}

impl Default for SpawnMeta {
    fn default() -> Self {
        SpawnMeta {
            spawnable: false,
            lift: 0.0,
            description: None,
        }
    }
}

/// Parse the spawn metadata out of USDA source.
pub fn parse_spawn_meta(src: &str) -> SpawnMeta {
    let mut meta = SpawnMeta::default();
    for line in src.lines() {
        if let Some((_, rhs)) = line.split_once("lunco:spawnLift") {
            if let Some(v) = rhs.split('=').nth(1) {
                if let Ok(f) = v.trim().parse::<f32>() {
                    meta.lift = f;
                }
            }
        } else if let Some((_, rhs)) = line.split_once("lunco:spawnable") {
            if let Some(v) = rhs.split('=').nth(1) {
                let v = v.trim();
                meta.spawnable = v.starts_with("true") || v.starts_with('1');
            }
        } else if let Some((_, rhs)) = line.split_once("lunco:description") {
            meta.description = parse_quoted(rhs);
        }
    }
    meta
}

/// The first double-quoted run to the right of an `=`.
fn parse_quoted(rhs: &str) -> Option<String> {
    let after_eq = rhs.split_once('=')?.1.trim();
    let rest = after_eq
        .strip_prefix('"')
        .or_else(|| after_eq.find('"').map(|i| &after_eq[i + 1..]))?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
