//! Modelica-side adapter to the canonical Twin journal.
//!
//! [`crate::document::ModelicaOp`] derives `Serialize`/`Deserialize`, so the
//! journal records the **real op** (lossless, replayable) via the typed
//! [`Journal::record_op`](lunco_twin_journal::Journal::record_op) — no
//! hand-written summary. This module supplies the [`OpPayload`] impl that names
//! the domain.
//!
//! Human-readable one-line summaries for a log / audit UI are produced
//! headlessly by [`lunco_twin_journal::JournalEntry::summary`] (generic over
//! the recorded JSON), so no domain- or UI-specific summarizer lives here.

use lunco_twin_journal::{DomainKind, OpPayload};

use crate::document::ModelicaOp;

impl OpPayload for ModelicaOp {
    fn domain(&self) -> DomainKind {
        DomainKind::Modelica
    }

    // `referenced_entities` stays the default empty set: an
    // `EntityRef` also needs the owning `DocumentId`, which the op alone
    // doesn't carry. Conflict-detection enrichment lands on the multi-user
    // replication path. (Same stance as the USD `OpPayload` impl.)
}
