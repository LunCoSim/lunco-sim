//! Versioned binary framing for bounded semantic session-input captures.

use crate::SessionInputRecord;
use bevy::prelude::Resource;
use lunco_command_contracts::SessionId;
use std::sync::Arc;

/// Maximum input records accepted by an in-memory capture and its archive.
pub const MAX_SESSION_INPUT_RECORDS: usize = 65_536;

/// Maximum encoded size of one session-input capture archive, including its
/// framing header.
pub const MAX_SESSION_INPUT_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;

const ARCHIVE_MAGIC: &[u8; 8] = b"LCSINP\0\0";
const ARCHIVE_VERSION: u16 = 1;
const HEADER_BYTES: usize = 8 + 2 + 4 + 4;
const MAX_ARCHIVE_PAYLOAD_BYTES: usize = MAX_SESSION_INPUT_ARCHIVE_BYTES - HEADER_BYTES;

#[derive(serde::Serialize, serde::Deserialize)]
enum ArchiveProducer {
    PhysicalController {
        session_id: SessionId,
    },
    ApiTransport {
        producer_id: u64,
    },
    Rhai {
        route: Option<lunco_core::RuntimeRoute>,
        actor: Option<lunco_core::GlobalEntityId>,
        producer_id: Option<u64>,
    },
    DirectCommand {
        producer_id: u64,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
enum ArchivePayload {
    PhysicalIntentFrame {
        intent_ids: Vec<String>,
    },
    SimulatedIntentChange {
        intent: String,
        held: bool,
        correlation_id: u64,
    },
    SemanticIntentEdge {
        intent: String,
        edge: String,
        correlation_id: u64,
    },
    RuntimeSpawn {
        entry_id: String,
        active_frame: lunco_core::GlobalEntityId,
        requested_position: [f64; 3],
        requested_rotation: Option<[f64; 4]>,
        correlation_id: u64,
        spawned_root: lunco_core::GlobalEntityId,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ArchiveRecord {
    producer: ArchiveProducer,
    target: lunco_core::GlobalEntityId,
    scene_generation: u64,
    effective_tick: u64,
    sequence: u64,
    payload: ArchivePayload,
}

impl From<&SessionInputRecord> for ArchiveRecord {
    fn from(record: &SessionInputRecord) -> Self {
        let producer = match record.producer {
            crate::SessionInputProducer::PhysicalController { session_id } => {
                ArchiveProducer::PhysicalController { session_id }
            }
            crate::SessionInputProducer::ApiTransport { producer_id } => {
                ArchiveProducer::ApiTransport { producer_id }
            }
            crate::SessionInputProducer::Rhai {
                route,
                actor,
                producer_id,
            } => ArchiveProducer::Rhai {
                route,
                actor,
                producer_id,
            },
            crate::SessionInputProducer::DirectCommand { producer_id } => {
                ArchiveProducer::DirectCommand { producer_id }
            }
        };
        let payload = match &record.payload {
            crate::SessionInputPayload::PhysicalIntentFrame { intent_ids } => {
                ArchivePayload::PhysicalIntentFrame {
                    intent_ids: intent_ids.clone(),
                }
            }
            crate::SessionInputPayload::SimulatedIntentChange {
                intent,
                held,
                correlation_id,
            } => ArchivePayload::SimulatedIntentChange {
                intent: intent.clone(),
                held: *held,
                correlation_id: *correlation_id,
            },
            crate::SessionInputPayload::SemanticIntentEdge {
                intent,
                edge,
                correlation_id,
            } => ArchivePayload::SemanticIntentEdge {
                intent: intent.clone(),
                edge: edge.clone(),
                correlation_id: *correlation_id,
            },
            crate::SessionInputPayload::RuntimeSpawn {
                entry_id,
                active_frame,
                requested_position,
                requested_rotation,
                correlation_id,
                spawned_root,
            } => ArchivePayload::RuntimeSpawn {
                entry_id: entry_id.clone(),
                active_frame: *active_frame,
                requested_position: *requested_position,
                requested_rotation: *requested_rotation,
                correlation_id: *correlation_id,
                spawned_root: *spawned_root,
            },
        };

        Self {
            producer,
            target: record.target,
            scene_generation: record.scene_generation,
            effective_tick: record.effective_tick,
            sequence: record.sequence,
            payload,
        }
    }
}

impl From<ArchiveRecord> for SessionInputRecord {
    fn from(record: ArchiveRecord) -> Self {
        let producer = match record.producer {
            ArchiveProducer::PhysicalController { session_id } => {
                crate::SessionInputProducer::PhysicalController { session_id }
            }
            ArchiveProducer::ApiTransport { producer_id } => {
                crate::SessionInputProducer::ApiTransport { producer_id }
            }
            ArchiveProducer::Rhai {
                route,
                actor,
                producer_id,
            } => crate::SessionInputProducer::Rhai {
                route,
                actor,
                producer_id,
            },
            ArchiveProducer::DirectCommand { producer_id } => {
                crate::SessionInputProducer::DirectCommand { producer_id }
            }
        };
        let payload = match record.payload {
            ArchivePayload::PhysicalIntentFrame { intent_ids } => {
                crate::SessionInputPayload::PhysicalIntentFrame { intent_ids }
            }
            ArchivePayload::SimulatedIntentChange {
                intent,
                held,
                correlation_id,
            } => crate::SessionInputPayload::SimulatedIntentChange {
                intent,
                held,
                correlation_id,
            },
            ArchivePayload::SemanticIntentEdge {
                intent,
                edge,
                correlation_id,
            } => crate::SessionInputPayload::SemanticIntentEdge {
                intent,
                edge,
                correlation_id,
            },
            ArchivePayload::RuntimeSpawn {
                entry_id,
                active_frame,
                requested_position,
                requested_rotation,
                correlation_id,
                spawned_root,
            } => crate::SessionInputPayload::RuntimeSpawn {
                entry_id,
                active_frame,
                requested_position,
                requested_rotation,
                correlation_id,
                spawned_root,
            },
        };

        Self {
            producer,
            target: record.target,
            scene_generation: record.scene_generation,
            effective_tick: record.effective_tick,
            sequence: record.sequence,
            payload,
        }
    }
}

/// Ordered, validated semantic records in the session-input capture format.
///
/// This archive stores input records only. It does not contain the admitted
/// scene/source manifest, initial runtime state, or a playback consumer, so it
/// is not a whole-session replay.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInputCaptureArchive {
    records: Arc<Vec<SessionInputRecord>>,
}

impl SessionInputCaptureArchive {
    /// Create an archive after validating each record and the total order.
    pub fn new(records: Vec<SessionInputRecord>) -> Result<Self, String> {
        validate_records(&records)?;
        Ok(Self {
            records: Arc::new(records),
        })
    }

    /// Create an archive from completed, shared capture records without
    /// copying their payloads on the caller's thread.
    pub fn from_shared(records: Arc<Vec<SessionInputRecord>>) -> Result<Self, String> {
        validate_records(&records)?;
        Ok(Self { records })
    }

    /// Validated records in their authoritative capture order.
    pub fn records(&self) -> &[SessionInputRecord] {
        self.records.as_slice()
    }

    /// Encode the current versioned archive framing and bounded bincode body.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        validate_records(&self.records)?;
        let record_count = u32::try_from(self.records.len()).map_err(|_| {
            "session input archive record count exceeds its header range".to_owned()
        })?;
        let wire_records = self
            .records
            .iter()
            .map(ArchiveRecord::from)
            .collect::<Vec<_>>();
        let config = bincode::config::standard().with_limit::<MAX_ARCHIVE_PAYLOAD_BYTES>();
        let payload = bincode::serde::encode_to_vec(&wire_records, config)
            .map_err(|error| format!("session input archive encode failed: {error}"))?;
        let payload_length = u32::try_from(payload.len())
            .map_err(|_| "session input archive payload exceeds its header range".to_owned())?;
        let total_length = HEADER_BYTES
            .checked_add(payload.len())
            .ok_or_else(|| "session input archive size overflowed".to_owned())?;
        if total_length > MAX_SESSION_INPUT_ARCHIVE_BYTES {
            return Err(format!(
                "session input archive exceeds its {} byte limit",
                MAX_SESSION_INPUT_ARCHIVE_BYTES
            ));
        }

        let mut bytes = Vec::with_capacity(total_length);
        bytes.extend_from_slice(ARCHIVE_MAGIC);
        bytes.extend_from_slice(&ARCHIVE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&record_count.to_le_bytes());
        bytes.extend_from_slice(&payload_length.to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    /// Decode one archive after checking its version, byte count, record
    /// count, payload consumption, record invariants, and order.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SESSION_INPUT_ARCHIVE_BYTES {
            return Err(format!(
                "session input archive exceeds its {} byte limit",
                MAX_SESSION_INPUT_ARCHIVE_BYTES
            ));
        }
        if bytes.len() < HEADER_BYTES {
            return Err("session input archive header is truncated".to_owned());
        }
        if &bytes[..ARCHIVE_MAGIC.len()] != ARCHIVE_MAGIC {
            return Err("session input archive magic is invalid".to_owned());
        }

        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != ARCHIVE_VERSION {
            return Err(format!(
                "session input archive version {version} is unsupported"
            ));
        }
        let record_count =
            u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
        if record_count > MAX_SESSION_INPUT_RECORDS {
            return Err(format!(
                "session input archive record count exceeds its {} record limit",
                MAX_SESSION_INPUT_RECORDS
            ));
        }
        let payload_length =
            u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]) as usize;
        if payload_length > MAX_ARCHIVE_PAYLOAD_BYTES
            || payload_length != bytes.len() - HEADER_BYTES
        {
            return Err(
                "session input archive payload length does not match its header".to_owned(),
            );
        }

        let payload = &bytes[HEADER_BYTES..];
        let config = bincode::config::standard().with_limit::<MAX_ARCHIVE_PAYLOAD_BYTES>();
        let (wire_records, consumed) =
            bincode::serde::decode_from_slice::<Vec<ArchiveRecord>, _>(payload, config)
                .map_err(|error| format!("session input archive decode failed: {error}"))?;
        if consumed != payload_length {
            return Err("session input archive payload contains trailing data".to_owned());
        }
        if wire_records.len() != record_count {
            return Err(format!(
                "session input archive record count {} does not match decoded count {}",
                record_count,
                wire_records.len()
            ));
        }
        let records = wire_records
            .into_iter()
            .map(SessionInputRecord::from)
            .collect();
        Self::new(records)
    }
}

fn validate_records(records: &[SessionInputRecord]) -> Result<(), String> {
    if records.len() > MAX_SESSION_INPUT_RECORDS {
        return Err(format!(
            "session input archive record count exceeds its {} record limit",
            MAX_SESSION_INPUT_RECORDS
        ));
    }

    let mut previous = None;
    let mut estimated_payload_bytes = 10usize;
    for record in records {
        record.validate()?;
        estimated_payload_bytes =
            estimated_payload_bytes.saturating_add(estimated_encoded_record_bytes(record));
        if estimated_payload_bytes > MAX_ARCHIVE_PAYLOAD_BYTES {
            return Err(format!(
                "session input archive exceeds its {} byte limit",
                MAX_SESSION_INPUT_ARCHIVE_BYTES
            ));
        }
        let order = (
            record.scene_generation,
            record.effective_tick,
            record.sequence,
        );
        if previous.is_some_and(|previous| order <= previous) {
            return Err(format!(
                "session input archive order must increase; received generation {}, tick {}, sequence {} after {:?}",
                order.0, order.1, order.2, previous
            ));
        }
        previous = Some(order);
    }
    Ok(())
}

/// Conservative encoded-size bound checked before materializing wire records.
/// The fixed allowance covers enum tags, identities, stamps, and numeric
/// values; variable text and sequence contents are added at their full size
/// plus the maximum bincode varint prefix.
fn estimated_encoded_record_bytes(record: &SessionInputRecord) -> usize {
    const FIXED_RECORD_BOUND: usize = 192;
    const VARINT_BOUND: usize = 10;

    let add_text = |total: usize, value: &str| {
        total
            .saturating_add(VARINT_BOUND)
            .saturating_add(value.len())
    };

    match &record.payload {
        crate::SessionInputPayload::PhysicalIntentFrame { intent_ids } => intent_ids
            .iter()
            .fold(FIXED_RECORD_BOUND + VARINT_BOUND, |total, intent| {
                add_text(total, intent)
            }),
        crate::SessionInputPayload::SimulatedIntentChange { intent, .. } => {
            add_text(FIXED_RECORD_BOUND, intent)
        }
        crate::SessionInputPayload::SemanticIntentEdge { intent, edge, .. } => {
            add_text(add_text(FIXED_RECORD_BOUND, intent), edge)
        }
        crate::SessionInputPayload::RuntimeSpawn { entry_id, .. } => {
            add_text(FIXED_RECORD_BOUND, entry_id)
        }
    }
}

/// Lifecycle of the latest durable session-input archive export.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionInputArchiveExportState {
    /// No archive export has been requested in this app session.
    #[default]
    Idle,
    /// A bounded archive export is being encoded or written.
    Pending,
    /// The archive was written and read back successfully.
    Complete,
    /// The archive could not be encoded, written, or verified.
    Failed,
}

/// Typed status and stale-completion guard for one session-input archive
/// export at a time.
#[derive(Resource, Debug, Default)]
pub struct SessionInputArchiveExportStatus {
    next_id: u64,
    state: SessionInputArchiveExportState,
    export_id: Option<u64>,
    capture_id: Option<u64>,
    last_exported_capture_id: Option<u64>,
    file_name: Option<String>,
    record_count: Option<u64>,
    byte_count: Option<u64>,
    failure: Option<String>,
}

impl SessionInputArchiveExportStatus {
    /// Start one export and return its monotonic app-local identity.
    pub fn begin(
        &mut self,
        capture_id: u64,
        file_name: String,
        record_count: usize,
    ) -> Result<u64, String> {
        if self.state == SessionInputArchiveExportState::Pending {
            return Err("a session input archive export is already pending".to_owned());
        }
        if capture_id == 0 {
            return Err("session input capture identity must be nonzero".to_owned());
        }
        if self
            .last_exported_capture_id
            .is_some_and(|last_exported| capture_id <= last_exported)
        {
            return Err("session input capture already has a durable archive".to_owned());
        }
        let export_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "session input archive export identity exhausted".to_owned())?;
        let record_count = u64::try_from(record_count)
            .map_err(|_| "session input record count exceeds the API integer range".to_owned())?;

        self.next_id = export_id;
        self.state = SessionInputArchiveExportState::Pending;
        self.export_id = Some(export_id);
        self.capture_id = Some(capture_id);
        self.file_name = Some(file_name);
        self.record_count = Some(record_count);
        self.byte_count = None;
        self.failure = None;
        Ok(export_id)
    }

    /// Finish only the currently pending export with the matching identity.
    pub fn complete(&mut self, export_id: u64, byte_count: u64) -> bool {
        if self.state != SessionInputArchiveExportState::Pending
            || self.export_id != Some(export_id)
        {
            return false;
        }
        self.state = SessionInputArchiveExportState::Complete;
        self.last_exported_capture_id = self.capture_id;
        self.byte_count = Some(byte_count);
        self.failure = None;
        true
    }

    /// Fail only the currently pending export with the matching identity.
    pub fn fail(&mut self, export_id: u64, failure: String) -> bool {
        if self.state != SessionInputArchiveExportState::Pending
            || self.export_id != Some(export_id)
        {
            return false;
        }
        self.state = SessionInputArchiveExportState::Failed;
        self.byte_count = None;
        self.failure = Some(failure);
        true
    }

    /// State of the latest export request.
    pub fn state(&self) -> SessionInputArchiveExportState {
        self.state
    }

    /// App-local identity of the latest export request, when present.
    pub fn export_id(&self) -> Option<u64> {
        self.export_id
    }

    /// Capture identity represented by the latest export request.
    pub fn capture_id(&self) -> Option<u64> {
        self.capture_id
    }

    /// Relative archive filename under the session-captures directory.
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// Number of captured records included in the latest request.
    pub fn record_count(&self) -> Option<u64> {
        self.record_count
    }

    /// Encoded archive size after a successful write and read-back check.
    pub fn byte_count(&self) -> Option<u64> {
        self.byte_count
    }

    /// Terminal export failure, when present.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ARCHIVE_MAGIC, ARCHIVE_VERSION, ArchivePayload, ArchiveProducer, ArchiveRecord,
        HEADER_BYTES, MAX_ARCHIVE_PAYLOAD_BYTES,
    };
    use crate::{
        MAX_SESSION_INPUT_ARCHIVE_BYTES, MAX_SESSION_INPUT_RECORDS,
        SessionInputArchiveExportStatus, SessionInputCaptureArchive, SessionInputPayload,
        SessionInputProducer, SessionInputRecord,
    };
    use lunco_command_contracts::SessionId;

    fn physical_record(tick: u64, sequence: u64) -> SessionInputRecord {
        SessionInputRecord {
            producer: SessionInputProducer::PhysicalController {
                session_id: SessionId(9),
            },
            target: lunco_core::GlobalEntityId::from_raw(42),
            scene_generation: 3,
            effective_tick: tick,
            sequence,
            payload: SessionInputPayload::PhysicalIntentFrame {
                intent_ids: vec!["forward".to_owned()],
            },
        }
    }

    fn valid_records() -> Vec<SessionInputRecord> {
        vec![
            physical_record(10, 1),
            SessionInputRecord {
                producer: SessionInputProducer::ApiTransport { producer_id: 8181 },
                target: lunco_core::GlobalEntityId::from_raw(42),
                scene_generation: 3,
                effective_tick: 10,
                sequence: 2,
                payload: SessionInputPayload::SemanticIntentEdge {
                    intent: "action".to_owned(),
                    edge: "pulse".to_owned(),
                    correlation_id: 301,
                },
            },
            SessionInputRecord {
                producer: SessionInputProducer::DirectCommand { producer_id: 7 },
                target: lunco_core::GlobalEntityId::from_raw(42),
                scene_generation: 3,
                effective_tick: 11,
                sequence: 1,
                payload: SessionInputPayload::RuntimeSpawn {
                    entry_id: "catalog-entry".to_owned(),
                    active_frame: lunco_core::GlobalEntityId::from_raw(43),
                    requested_position: [1.234_567_890_123, 20.000_000_000_007, -4.5],
                    requested_rotation: Some([0.0, 0.0, 0.125, 0.992_156_741_649_221_5]),
                    correlation_id: 401,
                    spawned_root: lunco_core::GlobalEntityId::from_raw(44),
                },
            },
            SessionInputRecord {
                producer: SessionInputProducer::Rhai {
                    route: Some(lunco_core::RuntimeRoute::application(
                        lunco_core::RuntimeCycle::Repl,
                    )),
                    actor: None,
                    producer_id: Some(88),
                },
                target: lunco_core::GlobalEntityId::from_raw(42),
                scene_generation: 3,
                effective_tick: 12,
                sequence: 1,
                payload: SessionInputPayload::SimulatedIntentChange {
                    intent: "forward".to_owned(),
                    held: true,
                    correlation_id: 302,
                },
            },
            SessionInputRecord {
                producer: SessionInputProducer::Rhai {
                    route: Some(lunco_core::RuntimeRoute::twin(
                        lunco_core::RuntimeCycle::Simulation,
                        3,
                    )),
                    actor: Some(lunco_core::GlobalEntityId::from_raw(98)),
                    producer_id: None,
                },
                target: lunco_core::GlobalEntityId::from_raw(42),
                scene_generation: 3,
                effective_tick: 13,
                sequence: 1,
                payload: SessionInputPayload::SemanticIntentEdge {
                    intent: "action".to_owned(),
                    edge: "pulse".to_owned(),
                    correlation_id: 303,
                },
            },
        ]
    }

    fn encode_wire_records(records: &[ArchiveRecord]) -> Vec<u8> {
        let config = bincode::config::standard().with_limit::<MAX_ARCHIVE_PAYLOAD_BYTES>();
        let payload =
            bincode::serde::encode_to_vec(records, config).expect("test wire records encode");
        let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
        bytes.extend_from_slice(ARCHIVE_MAGIC);
        bytes.extend_from_slice(&ARCHIVE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn session_input_capture_archive_round_trips_typed_records_and_f64_pose() {
        let expected = valid_records();
        let archive = SessionInputCaptureArchive::new(expected.clone())
            .expect("valid ordered records form an archive");

        let bytes = archive.to_bytes().expect("archive encodes");
        let decoded = SessionInputCaptureArchive::from_bytes(&bytes)
            .expect("archive framing and records decode");

        assert_eq!(decoded.records(), expected.as_slice());
        let SessionInputPayload::RuntimeSpawn {
            requested_position,
            requested_rotation,
            ..
        } = &decoded.records()[2].payload
        else {
            panic!("third archived record is the runtime spawn");
        };
        assert_eq!(
            *requested_position,
            [1.234_567_890_123, 20.000_000_000_007, -4.5]
        );
        assert_eq!(
            *requested_rotation,
            Some([0.0, 0.0, 0.125, 0.992_156_741_649_221_5])
        );
    }

    #[test]
    fn session_input_capture_archive_rejects_invalid_records_and_order() {
        let mut invalid = physical_record(10, 1);
        invalid.payload = SessionInputPayload::PhysicalIntentFrame {
            intent_ids: vec!["unknown-intent".to_owned()],
        };
        assert!(
            SessionInputCaptureArchive::new(vec![invalid])
                .expect_err("invalid payload is rejected")
                .contains("unknown intent")
        );

        let mut mismatched_producer = physical_record(10, 1);
        mismatched_producer.producer = SessionInputProducer::DirectCommand { producer_id: 7 };
        assert!(
            SessionInputCaptureArchive::new(vec![mismatched_producer])
                .expect_err("producer/payload mismatch is rejected")
                .contains("physical intent frame requires")
        );

        assert!(
            SessionInputCaptureArchive::new(vec![physical_record(10, 2), physical_record(10, 1)])
                .expect_err("archive order must be strictly increasing")
                .contains("order must increase")
        );
    }

    #[test]
    fn session_input_capture_archive_decoder_validates_untrusted_wire_records() {
        let invalid_intent = ArchiveRecord {
            producer: ArchiveProducer::PhysicalController {
                session_id: SessionId(9),
            },
            target: lunco_core::GlobalEntityId::from_raw(42),
            scene_generation: 3,
            effective_tick: 10,
            sequence: 1,
            payload: ArchivePayload::PhysicalIntentFrame {
                intent_ids: vec!["unknown-intent".to_owned()],
            },
        };
        assert!(
            SessionInputCaptureArchive::from_bytes(&encode_wire_records(&[invalid_intent]))
                .expect_err("decoded payload must be validated")
                .contains("unknown intent")
        );

        let mismatched_producer = ArchiveRecord {
            producer: ArchiveProducer::DirectCommand { producer_id: 7 },
            target: lunco_core::GlobalEntityId::from_raw(42),
            scene_generation: 3,
            effective_tick: 10,
            sequence: 1,
            payload: ArchivePayload::PhysicalIntentFrame {
                intent_ids: vec!["forward".to_owned()],
            },
        };
        assert!(
            SessionInputCaptureArchive::from_bytes(&encode_wire_records(&[mismatched_producer,]))
                .expect_err("decoded producer/payload pairing must be validated")
                .contains("physical intent frame requires")
        );
    }

    #[test]
    fn session_input_capture_archive_rejects_bad_header_version_and_size() {
        let bytes = SessionInputCaptureArchive::new(valid_records())
            .expect("valid archive")
            .to_bytes()
            .expect("archive encodes");

        let mut bad_magic = bytes.clone();
        bad_magic[0] ^= 0xff;
        assert!(
            SessionInputCaptureArchive::from_bytes(&bad_magic)
                .expect_err("bad magic is rejected")
                .contains("magic")
        );

        let mut unsupported_version = bytes.clone();
        unsupported_version[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(
            SessionInputCaptureArchive::from_bytes(&unsupported_version)
                .expect_err("unknown archive schema is rejected")
                .contains("version")
        );

        let mut excessive_record_count = bytes.clone();
        excessive_record_count[10..14]
            .copy_from_slice(&((MAX_SESSION_INPUT_RECORDS + 1) as u32).to_le_bytes());
        assert!(
            SessionInputCaptureArchive::from_bytes(&excessive_record_count)
                .expect_err("untrusted count is rejected before decoding")
                .contains("record count")
        );

        let mut mismatched_record_count = bytes.clone();
        mismatched_record_count[10..14].copy_from_slice(&2_u32.to_le_bytes());
        assert!(
            SessionInputCaptureArchive::from_bytes(&mismatched_record_count)
                .expect_err("header count must match decoded records")
                .contains("record count")
        );

        let mut trailing_data = bytes;
        trailing_data.push(0);
        let payload_length = (trailing_data.len() - HEADER_BYTES) as u32;
        trailing_data[14..18].copy_from_slice(&payload_length.to_le_bytes());
        assert!(
            SessionInputCaptureArchive::from_bytes(&trailing_data)
                .expect_err("trailing bytes are rejected")
                .contains("trailing data")
        );

        let oversized = vec![0; MAX_SESSION_INPUT_ARCHIVE_BYTES + 1];
        assert!(
            SessionInputCaptureArchive::from_bytes(&oversized)
                .expect_err("oversized archive is rejected before decode")
                .contains("byte limit")
        );
    }

    #[test]
    fn session_input_archive_export_status_rejects_stale_completions() {
        let mut status = SessionInputArchiveExportStatus::default();
        assert_eq!(status.state(), super::SessionInputArchiveExportState::Idle);
        assert!(
            status
                .begin(0, "zero-capture.lcsin".to_owned(), 0)
                .expect_err("capture identities are nonzero")
                .contains("must be nonzero")
        );
        assert_eq!(
            status
                .begin(71, "capture-71.lcsin".to_owned(), 4)
                .expect("first export starts"),
            1
        );
        assert!(!status.complete(0, 512));
        assert!(!status.fail(0, "stale failure".to_owned()));
        assert_eq!(
            status.state(),
            super::SessionInputArchiveExportState::Pending
        );
        assert!(status.complete(1, 512));
        assert_eq!(status.byte_count(), Some(512));
        assert_eq!(status.record_count(), Some(4));
        assert_eq!(status.capture_id(), Some(71));
        assert!(!status.complete(1, 513));

        assert!(
            status
                .begin(71, "duplicate.lcsin".to_owned(), 4)
                .expect_err("successful capture cannot be exported twice")
                .contains("already has a durable archive")
        );
        assert!(
            status
                .begin(70, "older-capture.lcsin".to_owned(), 4)
                .expect_err("capture identities cannot move behind the last export")
                .contains("already has a durable archive")
        );

        assert_eq!(
            status
                .begin(72, "capture-72.lcsin".to_owned(), 0)
                .expect("next export starts after completion"),
            2
        );
        assert!(status.fail(2, "write failed".to_owned()));
        assert_eq!(
            status.state(),
            super::SessionInputArchiveExportState::Failed
        );
        assert_eq!(status.failure(), Some("write failed"));
        assert_eq!(status.byte_count(), None);

        assert_eq!(
            status
                .begin(72, "capture-72-retry.lcsin".to_owned(), 0)
                .expect("failed capture export can be retried"),
            3
        );
    }

    #[test]
    fn session_input_capture_archive_bounds_variable_payloads_before_encoding() {
        let mut record = physical_record(10, 1);
        record.producer = SessionInputProducer::DirectCommand { producer_id: 7 };
        record.payload = SessionInputPayload::RuntimeSpawn {
            entry_id: "x".repeat(MAX_SESSION_INPUT_ARCHIVE_BYTES),
            active_frame: lunco_core::GlobalEntityId::from_raw(91),
            requested_position: [0.0; 3],
            requested_rotation: None,
            correlation_id: 77,
            spawned_root: lunco_core::GlobalEntityId::from_raw(92),
        };
        assert!(
            SessionInputCaptureArchive::new(vec![record])
                .expect_err("oversized payload is rejected before wire-record copies")
                .contains("byte limit")
        );
    }
}
