//! Asset-addressed Twin opening at the application integration boundary.
//! Assets resolves identity; Workspace retains scanning, supersession and commit.

use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;
use lunco_assets_core::SchemeRegistry;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_twin::{TwinError, TwinMode};
use lunco_workspace::open::{PendingTwinOpens, TwinOpenMode, enqueue_twin_scan};

/// Open the Twin identified by an exact registered manifest asset URI.
#[Command(default)]
pub struct OpenTwinAsset {
    /// Registered asset URI of `twin.toml`; no native root is guessed.
    pub manifest_asset: String,
}

#[on_command(OpenTwinAsset)]
fn on_open_twin_asset(
    trigger: On<OpenTwinAsset>,
    resolver: Res<SchemeRegistry>,
    mut pending: ResMut<PendingTwinOpens>,
) -> Result<Ack, String> {
    let source = trigger.event().manifest_asset.clone();
    if !lunco_assets_core::has_scheme(&source)
        || !source.ends_with(&format!("/{}", lunco_twin::MANIFEST_FILENAME))
    {
        return Err("OpenTwinAsset requires an exact registered twin.toml asset URI".into());
    }
    let label = std::path::PathBuf::from(&source);
    let failure_path = label.clone();
    let resolver = resolver.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let resolve = || -> Result<TwinMode, String> {
            let manifest = resolver
                .local_path(&source)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("Twin manifest has no local asset source: {source}"))?;
            if manifest
                .file_name()
                .is_none_or(|name| name != lunco_twin::MANIFEST_FILENAME)
            {
                return Err("resolved Twin source is not a twin.toml manifest".into());
            }
            let root = manifest
                .parent()
                .ok_or("Twin manifest has no parent directory")?;
            let twin = TwinMode::open(root).map_err(|error| error.to_string())?;
            if !matches!(twin, TwinMode::Twin(_)) {
                return Err("resolved asset root has no valid Twin manifest".into());
            }
            Ok(twin)
        };
        resolve().map_err(|message| TwinError::Io {
            path: failure_path,
            source: std::io::Error::other(message),
        })
    });
    enqueue_twin_scan(
        task,
        label,
        &mut pending,
        "OpenTwinAsset",
        None,
        TwinOpenMode::Replace,
    );
    Ok(Ack::new(OpId::new()))
}

register_commands!(on_open_twin_asset);
