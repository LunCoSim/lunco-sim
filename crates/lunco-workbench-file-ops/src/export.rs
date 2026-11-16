//! Application-owned immutable exports. Source retirement cannot change an admitted
//! snapshot. Dialogs and writes run asynchronously; completion names the exact file.

use bevy::prelude::*;
use bevy::tasks::{IoTaskPool, Task, futures_lite::future};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command};
use lunco_storage::{Storage, StorageHandle};
use lunco_workbench_file_dialog::{
    PickFollowUp, PickMode, PickResolved, PickedPath, SaveHint, request_pick,
};

/// Export an immutable UTF-8 snapshot. An empty path opens a save dialog.
#[Command(default)]
pub struct ExportText {
    pub suggested_name: String,
    pub content: String,
    pub path: String,
}

/// Bounded application-owned admission for each export preparation/I/O stage.
#[derive(Resource)]
pub struct ExportLimits {
    pub max_pending: usize,
}
impl Default for ExportLimits {
    fn default() -> Self {
        Self { max_pending: 4 }
    }
}

#[derive(Component)]
struct ExportSnapshot(std::sync::Arc<str>);

#[derive(Component)]
struct ExportWrite(Task<Result<String, String>>);

#[on_command(ExportText)]
fn export_text(
    trigger: On<ExportText>,
    mut commands: Commands,
    limits: Res<ExportLimits>,
    pending: Query<(), Or<(With<ExportSnapshot>, With<ExportWrite>)>>,
    #[cfg(target_arch = "wasm32")] picker: NonSend<lunco_workbench_file_dialog::BrowserPicker>,
) -> Result<Ack, String> {
    let event = trigger.event();
    if event.suggested_name.trim().is_empty() {
        return Err("Export needs a suggested filename".into());
    }
    if limits.max_pending == 0 || pending.iter().count() >= limits.max_pending {
        return Err(
            "Export queue is full. Finish or cancel an existing export before retrying.".into(),
        );
    }
    #[cfg(target_arch = "wasm32")]
    {
        if !event.path.is_empty() {
            return Err("Browser exports use downloads, not native paths".into());
        }
        lunco_workbench_file_dialog::download_file(&picker, &event.suggested_name, &event.content)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let snapshot = std::sync::Arc::<str>::from(event.content.as_str());
        if event.path.is_empty() {
            request_pick(
                &mut commands,
                PickMode::SaveFile(SaveHint {
                    suggested_name: Some(event.suggested_name.clone()),
                    ..Default::default()
                }),
                PickFollowUp::ExportText,
                ExportSnapshot(snapshot),
            );
        } else {
            begin_write(
                &mut commands,
                StorageHandle::File(event.path.clone().into()),
                snapshot,
            );
        }
    }
    Ok(Ack::new(OpId::new()))
}

fn begin_write(commands: &mut Commands, handle: StorageHandle, snapshot: std::sync::Arc<str>) {
    let name = handle.as_file_path().map(|path| path.display().to_string());
    let task = IoTaskPool::get().spawn(async move {
        let name = name.ok_or_else(|| "Text export requires a file destination".to_owned())?;
        lunco_storage::FileStorage::new()
            .write(&handle, snapshot.as_bytes())
            .await
            .map_err(|error| format!("Export {name}: {error}"))?;
        Ok(name)
    });
    commands.spawn(ExportWrite(task));
}

fn resolve_export(
    trigger: On<PickResolved>,
    snapshots: Query<&ExportSnapshot>,
    mut commands: Commands,
) {
    let event = trigger.event();
    if event.follow_up != PickFollowUp::ExportText {
        return;
    }
    let Ok(snapshot) = snapshots.get(event.request) else {
        return;
    };
    match &event.result {
        PickedPath::Path(handle) => begin_write(&mut commands, handle.clone(), snapshot.0.clone()),
        _ => commands.trigger(lunco_core::RuntimeError {
            name: "export-failed".into(),
            message: "Export destination is not a file".into(),
        }),
    }
}

fn finish_exports(
    mut bus: ResMut<lunco_status_core::status_bus::StatusBus>,
    mut writes: Query<(Entity, &mut ExportWrite)>,
    mut commands: Commands,
) {
    for (entity, mut write) in &mut writes {
        let Some(result) = future::block_on(future::poll_once(&mut write.0)) else {
            continue;
        };
        match result {
            Ok(path) => {
                info!("Exported {path}");
                bus.push(
                    "Export",
                    lunco_status_core::status_bus::StatusLevel::Info,
                    format!("Exported {path}"),
                );
            }
            Err(message) => commands.trigger(lunco_core::RuntimeError {
                name: "export-failed".into(),
                message,
            }),
        }
        commands.entity(entity).try_despawn();
    }
}

lunco_core::register_commands!(export_text);

pub(super) fn install(app: &mut App) {
    app.init_resource::<ExportLimits>();
    register_all_commands(app);
    app.add_observer(resolve_export)
        .add_systems(Update, finish_exports);
}
