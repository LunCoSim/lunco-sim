//! USD-backed scene catalogs and runtime spawn construction.
//!
//! This package owns asset enumeration, USD metadata reads, shader/source
//! listings, and the generic constructor for a runtime USD instance. Keeping
//! those concerns outside `lunco-scene-commands` means catalog changes do not
//! rebuild the larger command-handler crate, while the command plugin can still
//! install this package as a normal production dependency.

pub mod catalog;
pub mod spawn_meta;

use bevy::prelude::*;
use lunco_core::register_commands;

/// Installs catalog resources, discovery systems, and catalog-only commands.
pub struct SceneCatalogPlugin;

register_commands!(
    catalog::on_rescan_shaders,
    catalog::on_rescan_spawn_catalog,
);

impl Plugin for SceneCatalogPlugin {
    fn build(&self, app: &mut App) {
        // Catalog/source reads may fetch browser-served assets. Keep the
        // settings resource available when this package is used without the
        // GUI dataset plugin.
        lunco_settings::ensure_download_settings(app);
        register_all_commands(app);
        catalog::register_query(app);
        app.init_resource::<catalog::SpawnCatalog>();
        app.init_resource::<catalog::CatalogScan>();
        app.init_resource::<catalog::AssetMetaStore>();
        app.init_resource::<lunco_materials::ShaderCatalog>();
        app.add_systems(
            Update,
            (
                catalog::maintain_catalogs,
                catalog::drain_catalog_listing,
                catalog::drain_usd_scan,
            )
                .chain(),
        );
    }
}
