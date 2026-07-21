//! Body imagery, delivered by the **asset system** rather than by this crate.
//!
//! A celestial body's radius, GM and rotation are physics and live in code.
//! What it LOOKS like is data, and data that is far too big for git: the Blue
//! Marble mosaic and the LROC colour map are hundreds of megabytes of source
//! that get resized into 4K textures. So they are DECLARED in this crate's
//! `Assets.toml`, listed in Settings ▸ Downloadable data with everything else,
//! and fetched only when a user asks.
//!
//! There is deliberately no path to a texture anywhere in this crate. The
//! manifest entry names the body it belongs to (`[earth.body] naif_id = 399`),
//! and this module walks the registry looking for that sub-table. Adding
//! imagery for Mars is a manifest entry, not a code change; removing a dataset
//! removes the imagery. The engine never learns a filename.
//!
//! Three ways the bytes arrive, one code path for all of them:
//!
//! - downloaded now — the registry flips to `Installed` and the next frame binds
//! - cached from an earlier run — installed at startup, bound on the first frame
//! - shipped inside a package (`assets/.cache/textures/earth.png`) — likewise
//!   installed, because the registry probes every read root, not just the one it
//!   would have downloaded into
//!
//! Without any of them, a body renders its own colour (ocean blue, regolith
//! grey). That is a complete appearance, not a degraded one — see
//! `big_space_setup`'s note on why the untextured state is the default.

use bevy::prelude::*;
use lunco_materials::TextureLayer;
use serde::Deserialize;

use crate::globe_lod::{GlobeLod, GlobeTiles};
use crate::registry::CelestialBody;

/// The `[<key>.body]` sub-table of a manifest entry: which body these pixels
/// are of. Domain metadata rides with the declaration that produced the bytes —
/// `lunco-assets` carries it verbatim and never interprets it.
#[derive(Deserialize)]
struct BodyImageryDecl {
    /// NAIF id of the body (399 Earth, 301 Moon).
    naif_id: i32,
}

/// Datasets already bound, by NAIF id — so the scan is a no-op after the first
/// frame that finds each one, and a re-bind never churns the tile set.
#[derive(Resource, Default)]
pub(crate) struct BoundBodyImagery(Vec<i32>);

/// Declare this crate's datasets into the engine-wide registry that owns
/// fetching. Embedded, because a packaged binary has no source tree.
pub(crate) fn register_body_imagery_datasets(
    registry: Option<ResMut<lunco_assets::datasets::DatasetRegistry>>,
) {
    let Some(mut registry) = registry else {
        // No `DatasetsPlugin` (headless probe, test harness): the crate still
        // works, it just has nothing to offer for download.
        return;
    };
    registry.register(include_str!("../Assets.toml"), "celestial");
}

/// The NAIF id a dataset declares imagery for, or `None` when it declares none.
///
/// Split out from the system so the *decision* is testable without an ECS
/// world: everything downstream is a query and a look assignment.
fn declared_body(entry: &lunco_assets::datasets::DatasetEntry) -> Option<i32> {
    match entry.spec.domain::<BodyImageryDecl>("body") {
        Some(Ok(d)) => Some(d.naif_id),
        // A typo'd `[*.body]` table must be loud: it reads as "this dataset
        // simply has no imagery", which is indistinguishable from a texture
        // that silently never appears.
        Some(Err(e)) => {
            warn!(
                "[celestial] dataset '{}' has a malformed [body] table: {e}",
                entry.key
            );
            None
        }
        None => None,
    }
}

/// Bind every installed dataset that names a body to that body's globe.
///
/// Runs before the LOD update so a look and the tiles carrying it land in the
/// same frame, exactly as [`adopt_authored_body_look`](crate::big_space_setup::adopt_authored_body_look)
/// does — and loses to it deliberately: a scene that AUTHORS a material on the
/// body prim has said what it wants, and a downloaded default must not overrule
/// content.
pub(crate) fn bind_dataset_body_imagery(
    registry: Option<Res<lunco_assets::datasets::DatasetRegistry>>,
    asset_server: Res<AssetServer>,
    mut bound: ResMut<BoundBodyImagery>,
    mut q_globes: Query<(&CelestialBody, &mut GlobeLod, &mut GlobeTiles)>,
    mut commands: Commands,
) {
    let Some(registry) = registry else { return };
    if q_globes.is_empty() {
        return;
    }
    for entry in registry.entries() {
        if !entry.state.is_installed() {
            continue;
        }
        let Some(naif_id) = declared_body(entry) else {
            continue;
        };
        if bound.0.contains(&naif_id) {
            continue;
        }
        let Some((_, mut lod, mut tiles)) = q_globes
            .iter_mut()
            .find(|(body, _, _)| body.ephemeris_id == naif_id)
        else {
            continue; // body not in this scene — nothing to dress
        };

        // The URI, not the path: `lunco://` searches the packed cache and the
        // shared pool in turn, so this one string resolves the same whether the
        // file shipped with the build or was downloaded a moment ago.
        let image = asset_server.load(entry.artifact_uri());
        lod.look = lod.look.clone().with_texture(TextureLayer::Albedo, image);
        // Resident tiles carry the OLD look (cloned onto each at spawn). Drop
        // them; `update_globe_lod` respawns the same set with the new one.
        for (_, e) in tiles.resident.drain() {
            commands.entity(e).try_despawn();
        }
        for (e, _) in tiles.retiring.drain(..) {
            commands.entity(e).try_despawn();
        }
        bound.0.push(naif_id);
        info!(
            "[celestial] body {naif_id} took its imagery from dataset '{}'",
            entry.key
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_assets::datasets::{DatasetRegistry, DatasetScope};

    /// The shipped manifest must actually declare which body each texture is
    /// of. This is the whole binding: drop the `[earth.body]` table and Earth
    /// silently renders untextured forever, with nothing in the code to notice.
    #[test]
    fn the_shipped_manifest_binds_its_textures_to_bodies() {
        let mut r = DatasetRegistry::default();
        assert!(r.register(include_str!("../Assets.toml"), "celestial") >= 2);
        let bodies: Vec<(String, Option<i32>)> = r
            .entries()
            .iter()
            .map(|e| (e.key.clone(), declared_body(e)))
            .collect();
        assert!(
            bodies.contains(&("earth".to_string(), Some(399))),
            "earth imagery must name NAIF 399: {bodies:?}"
        );
        assert!(
            bodies.contains(&("moon".to_string(), Some(301))),
            "moon imagery must name NAIF 301: {bodies:?}"
        );
    }

    /// What the renderer loads is the PROCESSED texture, addressed through
    /// `lunco://` — the scheme that searches the packed cache and the shared
    /// pool — never the multi-hundred-megabyte source download.
    #[test]
    fn imagery_is_addressed_as_a_library_uri_not_a_cache_path() {
        let mut r = DatasetRegistry::default();
        r.register(include_str!("../Assets.toml"), "celestial");
        let earth = r
            .entries()
            .iter()
            .find(|e| e.key == "earth")
            .expect("earth declared");
        assert_eq!(earth.scope, DatasetScope::Engine);
        assert_eq!(earth.artifact_uri(), "lunco://textures/earth.png");
        assert!(
            earth.path.ends_with("textures/earth_source.jpg"),
            "the download is the source, not the artifact: {:?}",
            earth.path
        );
    }

    /// A dataset with no `[*.body]` table is simply not imagery — the ephemeris
    /// CSVs share this registry and must not be mistaken for textures.
    #[test]
    fn a_dataset_without_a_body_table_declares_no_imagery() {
        let mut r = DatasetRegistry::default();
        r.register(
            r#"
[some_vectors]
name = "Vectors"
url = "https://example.invalid/v.csv"
dest = "ephemeris/v.csv"
"#,
            "other",
        );
        assert_eq!(declared_body(&r.entries()[0]), None);
    }
}
