//! Bounded durable export for completed session-input captures.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

use bevy::{log::warn, prelude::*, tasks::IoTaskPool};
use lunco_command_contracts::{Ack, OpId, Reject};
use lunco_core::{Command, on_command, register_commands};
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_core_session::{
    SessionInputArchiveExportState, SessionInputArchiveExportStatus, SessionInputCaptureArchive,
    SessionInputStream, SessionInputStreamState,
};
use lunco_hooks::HookValue;
use lunco_storage::{FileStorage, Storage, StorageError, StorageHandle};

/// App-owned directory for exported session-input archives.
#[derive(Resource, Clone, Debug)]
struct SessionInputArchiveExportDirectory(PathBuf);

impl Default for SessionInputArchiveExportDirectory {
    fn default() -> Self {
        Self(lunco_settings::user_config_dir().join("session-captures"))
    }
}

#[derive(Clone, Debug)]
struct ArchiveExportCompletion {
    export_id: u64,
    result: Result<u64, String>,
}

#[derive(Resource, Clone, Debug, Default)]
struct ArchiveExportCompletions(Arc<Mutex<VecDeque<ArchiveExportCompletion>>>);

/// Request a durable archive for a completed session-input capture.
///
/// `file_stem` is a simple filename stem. The service adds a unique operation
/// suffix and `.lcsin`, and writes only under the application session-captures
/// directory.
#[Command(default)]
pub struct ExportSessionInputCapture {
    /// Filename stem containing only ASCII letters, digits, `_`, or `-`.
    pub file_stem: String,
}

fn valid_file_stem(stem: &str) -> bool {
    (1..=64).contains(&stem.len())
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn archive_handle(
    directory: &SessionInputArchiveExportDirectory,
    file_name: &str,
) -> StorageHandle {
    StorageHandle::File(directory.0.join(file_name))
}

fn persist_and_verify_archive(
    handle: &StorageHandle,
    bytes: &[u8],
    expected_records: &[lunco_core_session::SessionInputRecord],
) -> Result<u64, String> {
    let storage = FileStorage::new();
    storage
        .write_new_sync(handle, bytes)
        .map_err(|error| match error {
            StorageError::AlreadyExists => "session input archive already exists".to_owned(),
            error => format!("session input archive write failed: {error}"),
        })?;
    let stored = storage
        .read_sync(handle)
        .map_err(|error| format!("session input archive read-back failed: {error}"))?;
    let decoded = SessionInputCaptureArchive::from_bytes(&stored)
        .map_err(|error| format!("session input archive read-back is invalid: {error}"))?;
    if decoded.records() != expected_records {
        return Err(
            "session input archive read-back did not match the captured records".to_owned(),
        );
    }
    u64::try_from(stored.len())
        .map_err(|_| "session input archive byte count exceeds the API integer range".to_owned())
}

#[on_command(ExportSessionInputCapture)]
fn on_export_session_input_capture(
    trigger: On<ExportSessionInputCapture>,
    stream: Res<SessionInputStream>,
    mut status: ResMut<SessionInputArchiveExportStatus>,
    mut admission: ResMut<AsyncWorkAdmission>,
    directory: Res<SessionInputArchiveExportDirectory>,
    completions: Res<ArchiveExportCompletions>,
) -> Result<Ack, Reject> {
    let file_stem = &trigger.event().file_stem;
    if !valid_file_stem(file_stem) {
        return Err(Reject::InvalidOp(
            "session input archive file_stem must be 1 to 64 ASCII letters, digits, '_' or '-'"
                .to_owned(),
        ));
    }
    if stream.state() != SessionInputStreamState::Complete {
        return Err(Reject::InvalidOp(
            "session input archive export requires a completed capture".to_owned(),
        ));
    }
    if status.state() == SessionInputArchiveExportState::Pending {
        return Err(Reject::InvalidOp(
            "a session input archive export is already pending".to_owned(),
        ));
    }

    let capture_id = stream.capture_id().ok_or_else(|| {
        Reject::InvalidOp("completed session input capture has no identity".to_owned())
    })?;
    let records = stream.shared_records();
    let record_count = u64::try_from(records.len()).map_err(|_| {
        Reject::InvalidOp("session input record count exceeds the API integer range".to_owned())
    })?;
    let file_nonce = OpId::new().0;
    let file_name = format!("{file_stem}-{file_nonce}.lcsin");
    let export_id = status
        .begin(capture_id, file_name.clone(), records.len())
        .map_err(Reject::InvalidOp)?;

    let key = AsyncWorkKey::new(
        AsyncWorkKind::SessionInputArchiveExport,
        0,
        u128::from(export_id),
        0,
        export_id,
    );
    let output_directory = directory.clone();
    let output_file_name = file_name.clone();
    let completion_queue = Arc::clone(&completions.0);
    let submitted = admission.submit(AsyncWorkPriority::Background, key, move || {
        let Some(io_pool) = IoTaskPool::try_get() else {
            push_completion(
                &completion_queue,
                ArchiveExportCompletion {
                    export_id,
                    result: Err("session input archive requires the I/O task pool".to_owned()),
                },
            );
            return;
        };
        let archive = match SessionInputCaptureArchive::from_shared(Arc::clone(&records)) {
            Ok(archive) => archive,
            Err(error) => {
                push_completion(
                    &completion_queue,
                    ArchiveExportCompletion {
                        export_id,
                        result: Err(error),
                    },
                );
                return;
            }
        };
        let bytes = match archive.to_bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                push_completion(
                    &completion_queue,
                    ArchiveExportCompletion {
                        export_id,
                        result: Err(error),
                    },
                );
                return;
            }
        };
        let handle = archive_handle(&output_directory, &output_file_name);
        io_pool
            .spawn(async move {
                let result = persist_and_verify_archive(&handle, &bytes, records.as_slice());
                push_completion(
                    &completion_queue,
                    ArchiveExportCompletion { export_id, result },
                );
            })
            .detach();
    });

    if let Err(error) = submitted {
        let failure = format!("session input archive work was not admitted: {error:?}");
        status.fail(export_id, failure.clone());
        return Err(Reject::InvalidOp(failure));
    }

    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([
            ("state", HookValue::str("pending")),
            ("export_id", HookValue::UInt(export_id)),
            ("capture_id", HookValue::UInt(capture_id)),
            ("file_name", HookValue::str(file_name)),
            ("record_count", HookValue::UInt(record_count)),
        ]),
    ))
}

fn push_completion(
    queue: &Mutex<VecDeque<ArchiveExportCompletion>>,
    completion: ArchiveExportCompletion,
) {
    queue
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push_back(completion);
}

fn apply_archive_export_completions(
    completions: Res<ArchiveExportCompletions>,
    mut status: ResMut<SessionInputArchiveExportStatus>,
) {
    let completed = {
        let mut queue = completions.0.lock().unwrap_or_else(PoisonError::into_inner);
        queue.drain(..).collect::<Vec<_>>()
    };

    for completion in completed {
        let accepted = match completion.result {
            Ok(byte_count) => status.complete(completion.export_id, byte_count),
            Err(failure) => status.fail(completion.export_id, failure.clone()),
        };
        if !accepted {
            warn!(
                export_id = completion.export_id,
                "[session-input-archive] ignored stale export completion"
            );
        }
    }
}

/// Install the durable archive command and its bounded async result commit.
pub(super) fn install(app: &mut App) {
    if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
        app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
    }
    app.init_resource::<SessionInputArchiveExportDirectory>()
        .init_resource::<ArchiveExportCompletions>()
        .add_systems(Update, apply_archive_export_completions);
    register_all_commands(app);
}

register_commands!(on_export_session_input_capture);
