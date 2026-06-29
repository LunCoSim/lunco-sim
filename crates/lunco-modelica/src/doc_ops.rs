//! Egui-free Modelica document op application.
//!
//! The single-op apply funnel (`apply_one_op_as`) plus its shared,
//! egui-free helper closure: the apply kernel, the deferred-structural-op
//! queue, and the journal-recording shim. Lifted out of the (egui-gated)
//! `ui::panels::canvas_diagram::ops` module so the headless / server build
//! and the core `api::*` command observers can apply ops without pulling in
//! egui. The batch `apply_ops` path (egui-using: pins tabs, wakes the canvas
//! panel) stays in `ops.rs` and calls back into these helpers.

use bevy::prelude::*;

use crate::document::ModelicaOp;
use crate::state::ModelicaDocumentRegistry;

/// Whether `op` mutates the source in a way that requires the host's
/// AST to be reparsed *before* the op can be applied — `ReplaceSource`
/// is a text-edit op (no inline AST mutation), so the next op needs a
/// fresh parse to look up the class it just renamed/replaced. Same
/// list applies to single-op and batch paths because both are reading
/// the same syntax cache.
pub(crate) fn op_needs_fresh_ast_pre_apply(op: &ModelicaOp) -> bool {
    matches!(
        op,
        ModelicaOp::AddClass { .. }
            | ModelicaOp::RemoveClass { .. }
            | ModelicaOp::AddShortClass { .. }
            | ModelicaOp::AddVariable { .. }
            | ModelicaOp::RemoveVariable { .. }
            | ModelicaOp::AddEquation { .. }
            | ModelicaOp::AddIconGraphic { .. }
            | ModelicaOp::AddDiagramGraphic { .. }
            | ModelicaOp::SetExperimentAnnotation { .. }
            | ModelicaOp::ReplaceSource { .. }
    )
}

/// Single-op kernel: `host.apply(op)` → on success, waive the AST
/// debounce so the async engine sync picks the doc up on the next
/// tick. **No synchronous reparse anywhere.** Both pre- and
/// post-apply sync reparses have been removed from this path so
/// the write side never blocks the UI thread, regardless of op
/// kind or doc size.
///
/// Freshness contract:
///
/// - Pre-apply: kernel assumes the syntax cache is fresh enough
///   for `host.apply` to use. Callers (`apply_one_op_as`,
///   `apply_ops`) enforce this — when the op needs a fresh AST
///   and the cache is stale, they defer the op into
///   [`PendingStructuralOps`] and drain it after the async parse
///   lands. The kernel itself never reparses.
/// - Post-apply: structured ops install `FreshAst::Mutated`
///   inline, so same-frame readers see fresh. Text ops mark the
///   cache stale; `drive_engine_sync` reparses off-thread and
///   fires `DocumentChanged`. UI subscribers react then.
///
/// In debug builds, kernel debug-asserts the pre-apply contract
/// to catch any caller that bypasses the gate and lands a
/// structural op against stale syntax — the apply would mutate a
/// stale tree and emit a wrong patch.
///
/// Both [`apply_one_op_as`] and `apply_ops` funnel through here so
/// the journal-pair shape can't drift between single-op and batch
/// paths.
///
/// Returns `(host.apply result, optional (forward, backward) journal
/// summaries)`. Caller is responsible for journal-record +
/// `mark_changed` on the registry — those need access to the
/// registry / world that the kernel doesn't hold.
pub(crate) fn apply_one_op_kernel(
    host: &mut lunco_doc::DocumentHost<crate::document::ModelicaDocument>,
    op: ModelicaOp,
) -> (
    Result<lunco_doc::Ack, lunco_doc::Reject>,
    Option<(serde_json::Value, serde_json::Value)>,
) {
    debug_assert!(
        !op_needs_fresh_ast_pre_apply(&op) || !host.document().syntax_is_stale(),
        "apply_one_op_kernel: op {:?} requires fresh AST but syntax cache is stale — caller must defer through PendingStructuralOps",
        std::mem::discriminant(&op),
    );
    let forward = crate::journal::summarize_op(&op);
    let result = host.apply(op);
    let pair = if result.is_ok() {
        host.document_mut().waive_ast_debounce();
        host.last_applied_inverse()
            .map(crate::journal::summarize_op)
            .map(|backward| (forward, backward))
    } else {
        None
    };
    (result, pair)
}

/// Queue of structural ops that arrived while their target doc had
/// a stale syntax cache. Drained by [`drain_pending_structural_ops`]
/// after the async engine sync lands a fresh parse.
///
/// Replaces the old synchronous `refresh_ast_now()` in the kernel
/// pre-apply path: instead of blocking the main thread to reparse
/// before applying, the op waits one async parse cycle (typically
/// hundreds of milliseconds) and lands as soon as the cache is
/// fresh again. The user perceives a normal latency on their click,
/// the UI never freezes.
///
/// Per-doc FIFO order is preserved so dependent ops in the same
/// burst (e.g. AddClass then AddVariable in that class) apply in
/// the order they were issued.
#[derive(bevy::prelude::Resource, Default)]
pub struct PendingStructuralOps {
    pub(crate) queue: std::collections::HashMap<
        lunco_doc::DocumentId,
        std::collections::VecDeque<(ModelicaOp, lunco_twin_journal::AuthorTag)>,
    >,
}

fn deferred_ack() -> lunco_doc::Ack {
    let mut ack = lunco_doc::Ack::default();
    ack.assigned = serde_json::json!({ "deferred": true });
    ack
}

/// Exclusive system that retries queued structural ops once their
/// target doc's syntax cache catches up. Cheap when the queue is
/// empty (steady state); only does work after a stretch of typing
/// preceded a structural op.
pub fn drain_pending_structural_ops(world: &mut bevy::prelude::World) {
    // Phase 1: identify docs whose cache is fresh and have a non-empty queue.
    let fresh_docs: Vec<lunco_doc::DocumentId> = {
        let Some(registry) = world.get_resource::<ModelicaDocumentRegistry>() else {
            return;
        };
        let Some(pending) = world.get_resource::<PendingStructuralOps>() else {
            return;
        };
        pending
            .queue
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .filter_map(|(doc, _)| {
                registry
                    .host(*doc)
                    .map(|h| !h.document().syntax_is_stale())
                    .unwrap_or(false)
                    .then_some(*doc)
            })
            .collect()
    };
    if fresh_docs.is_empty() {
        return;
    }

    // Phase 2: drain queues into a local Vec, dropping the resource borrow.
    let ready: Vec<(lunco_doc::DocumentId, ModelicaOp, lunco_twin_journal::AuthorTag)> = {
        let mut pending = world.resource_mut::<PendingStructuralOps>();
        let mut out = Vec::new();
        for doc in &fresh_docs {
            if let Some(q) = pending.queue.get_mut(doc) {
                while let Some((op, author)) = q.pop_front() {
                    out.push((*doc, op, author));
                }
                if q.is_empty() {
                    pending.queue.remove(doc);
                }
            }
        }
        out
    };

    // Phase 3: re-enter the public apply path. With a fresh cache,
    // the deferred gate will pass and the kernel applies normally.
    for (doc, op, author) in ready {
        if let Err(reject) = apply_one_op_as(world, doc, op, author) {
            // A deferred op that fails on replay used to vanish silently,
            // leaving the document in a half-applied state with no trace.
            warn!("[modelica] deferred op for {doc:?} rejected on replay: {reject:?}");
        }
    }
}

/// Record one (forward, inverse) op pair into the canonical Twin
/// journal. Caller drops the registry borrow before invoking — the
/// journal resource lives on `&World`, not `&mut`. Single source of
/// truth so the recording shape doesn't drift between single-op and
/// batch paths either.
pub(crate) fn record_journal_entry(
    world: &World,
    doc_id: lunco_doc::DocumentId,
    author: lunco_twin_journal::AuthorTag,
    forward: serde_json::Value,
    backward: serde_json::Value,
) {
    if let Some(journal) = world.get_resource::<lunco_doc_bevy::JournalResource>() {
        journal.with_write(|j| {
            j.record_op_value(
                author,
                doc_id,
                lunco_twin_journal::DomainKind::Modelica,
                forward,
                backward,
                None,
            );
        });
    }
}

/// Apply a single op through `host.apply` AND record the (forward,
/// inverse) pair to the canonical Twin journal in one funnel.
///
/// Replaces the direct-`host.apply` pattern that several API command
/// observers used to bypass the journal-recording path. Returns the
/// `host.apply` result so callers can branch on success/failure
/// exactly as before.
///
/// Side effects on success — guaranteed by [`apply_one_op_kernel`]:
/// - `waive_ast_debounce()` so `drive_engine_sync` reparses promptly.
/// - `registry.mark_changed(doc)` (queues a `DocumentChanged` event).
/// - One canonical journal entry recorded with the supplied `author`.
///
/// Deferral: when `op` needs a fresh AST to apply (see
/// [`op_needs_fresh_ast_pre_apply`]) and the doc's syntax cache is
/// stale, the op is pushed into [`PendingStructuralOps`] and a
/// `deferred_ack` is returned immediately. The op then lands on
/// the next [`drain_pending_structural_ops`] tick after the async
/// parse completes. From the caller's perspective the op is
/// accepted; the journal entry, `DocumentChanged`, and any other
/// side-effects fire on actual apply, not on enqueue.
pub fn apply_one_op_as(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    op: ModelicaOp,
    author: lunco_twin_journal::AuthorTag,
) -> Result<lunco_doc::Ack, lunco_doc::Reject> {
    if op_needs_fresh_ast_pre_apply(&op) {
        let stale = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc_id))
            .map(|h| h.document().syntax_is_stale())
            .unwrap_or(false);
        if stale {
            if let Some(mut registry) = world.get_resource_mut::<ModelicaDocumentRegistry>() {
                if let Some(host) = registry.host_mut(doc_id) {
                    host.document_mut().waive_ast_debounce();
                }
            }
            world
                .resource_mut::<PendingStructuralOps>()
                .queue
                .entry(doc_id)
                .or_default()
                .push_back((op, author));
            return Ok(deferred_ack());
        }
    }

    let (result, pair) = {
        let Some(mut registry) = world.get_resource_mut::<ModelicaDocumentRegistry>() else {
            return Err(lunco_doc::Reject::InvalidOp(
                "ModelicaDocumentRegistry resource missing".into(),
            ));
        };
        let Some(host) = registry.host_mut(doc_id) else {
            return Err(lunco_doc::Reject::InvalidOp(format!(
                "doc {doc_id:?} not in registry"
            )));
        };
        let (result, pair) = apply_one_op_kernel(host, op);
        if result.is_ok() {
            registry.mark_changed(doc_id);
        }
        (result, pair)
    };

    if let Some((forward, backward)) = pair {
        record_journal_entry(world, doc_id, author, forward, backward);
    }
    result
}

/// Apply a **batch** of ops to `doc_id` with the canonical journal +
/// read-only handling. This is the egui-free core of the canvas's
/// `apply_ops`: the UI wrapper layers tab-pinning, timing probes, and the
/// projection/repaint flourishes on top, but the actual mutation funnels
/// through here so the API path (`api::on_apply_modelica_ops`) and a
/// headless server apply ops identically to the editor.
///
/// Batch-atomic deferral: if any op needs a fresh AST (see
/// [`op_needs_fresh_ast_pre_apply`]) and the doc's syntax cache is stale, the
/// WHOLE batch is queued (in order) into [`PendingStructuralOps`] so
/// dependent intra-batch ops (e.g. `AddClass` then `AddVariable` in that
/// class) apply against the same freshly-parsed tree on the next
/// [`drain_pending_structural_ops`] tick.
///
/// Returns whether any op applied synchronously (`false` on full-batch
/// deferral or an all-no-op / registry-missing batch).
pub fn apply_ops_as(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    ops: Vec<ModelicaOp>,
    author: lunco_twin_journal::AuthorTag,
) -> bool {
    // Deferral gate: queue the whole batch in order if any op reads the AST
    // and the syntax cache is stale (keeps intra-batch ops on one fresh tree).
    if ops.iter().any(op_needs_fresh_ast_pre_apply) {
        let stale = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc_id))
            .map(|h| h.document().syntax_is_stale())
            .unwrap_or(false);
        if stale {
            if let Some(mut registry) = world.get_resource_mut::<ModelicaDocumentRegistry>() {
                if let Some(host) = registry.host_mut(doc_id) {
                    host.document_mut().waive_ast_debounce();
                }
            }
            let mut pending = world.resource_mut::<PendingStructuralOps>();
            let queue = pending.queue.entry(doc_id).or_default();
            for op in ops {
                queue.push_back((op, author.clone()));
            }
            return false;
        }
    }

    // Preload any newly-referenced MSL class on a background task so the
    // engine session is warm by the time projection re-runs. Fire-and-forget;
    // rumoca's content-hash artifact cache dedupes repeated requests.
    for op in &ops {
        if let ModelicaOp::AddComponent { decl, .. } = op {
            if decl.type_name.starts_with("Modelica.") {
                let qualified = decl.type_name.clone();
                bevy::tasks::AsyncComputeTaskPool::get()
                    .spawn(async move {
                        let _ = crate::class_cache::peek_or_load_msl_class_blocking(&qualified);
                    })
                    .detach();
            }
        }
    }

    let n = ops.len();
    let mut any_applied = false;
    let mut hit_read_only = false;
    let mut journal_pairs: Vec<(serde_json::Value, serde_json::Value)> = Vec::new();
    {
        let Some(mut registry) = world.get_resource_mut::<ModelicaDocumentRegistry>() else {
            bevy::log::warn!("[doc_ops] apply_ops: registry missing ({n} op(s))");
            return false;
        };
        let Some(host) = registry.host_mut(doc_id) else {
            bevy::log::warn!("[doc_ops] apply_ops: doc {doc_id:?} not in registry ({n} op(s))");
            return false;
        };
        for op in ops {
            let (result, pair) = apply_one_op_kernel(host, op);
            match result {
                Ok(_) => {
                    any_applied = true;
                    if let Some(p) = pair {
                        journal_pairs.push(p);
                    }
                }
                // Document layer rejects mutations on read-only origins (MSL
                // drill-in, bundled library) — surface ONE banner per batch.
                Err(lunco_doc::Reject::ReadOnly) => hit_read_only = true,
                Err(e) => bevy::log::warn!("[doc_ops] op failed: {e}"),
            }
        }
        if any_applied {
            registry.mark_changed(doc_id);
        }
    }

    // Record captured op pairs into the canonical Twin journal (one lock).
    for (forward, backward) in journal_pairs {
        record_journal_entry(world, doc_id, author.clone(), forward, backward);
    }

    if hit_read_only {
        if let Some(mut cs) = world.get_resource_mut::<crate::state::CompileStates>() {
            // Don't clobber a real compile error.
            if cs.error_for(doc_id).is_none() {
                cs.set_error(
                    doc_id,
                    "Read-only library tab — edits rejected. \
                     Use File → Duplicate to Workspace to create an \
                     editable copy."
                        .to_string(),
                );
            }
        }
    }

    any_applied
}
