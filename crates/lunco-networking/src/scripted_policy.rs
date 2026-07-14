//! **Scripted-policy activation** — compile rhai policies into the hook registry.
//!
//! The hook substrate ([`lunco_hooks`]) lets internal decisions be authored in
//! rhai: the convergent **merge** order ([`lunco_twin_journal::MergePolicy`]) at
//! [`MERGE_SEAM`], the **authorization** gate ([`lunco_core::session::AUTHORIZE_HOOK`]),
//! and per-vehicle **drive kernels** (a `lunco:driveKernel` hook id in
//! [`lunco_core::kernels::DriveMix`]). A scripted policy is only correct if **every
//! peer runs the identical one** — most sharply for the merge policy, whose
//! determinism contract is that all peers linearize history the same way.
//!
//! Distribution is **not** a bespoke plane. A policy is a `LunCoPolicy` **USD prim**
//! (`lunco:policy:{seam,entry,source,deterministic}`); authoring one is an ordinary
//! USD doc op, so it rides the journal — persisted, per-author, RBAC-gated, and
//! convergent — and every peer recomposes the identical stage. This module is only
//! the **activation** half: [`project_policies`] takes the desired set (read from the
//! composed stage by the projector in the assembly crate) and (de)registers the
//! rhai hooks to match. [`ScriptedPolicyRegistry`] is the derived "currently active"
//! cache, not an authoritative broadcast source.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use lunco_doc_bevy::JournalResource;

/// The reserved policy **seam** (hook id) that drives the journal's convergent
/// merge order: a policy authored under this seam flips the journal's
/// [`MergeStrategy`] to `Scripted(seam)`. Every other seam is an open, self-
/// registering hook id (control-authority, drive-kernel, or any future decision
/// point) — no closed taxonomy (`feedback_less_rust_more_dynamic_registries`).
pub const MERGE_SEAM: &str = "journal.merge.order";

/// One scripted policy: a rhai `source` whose `entry` function fills the hook at
/// `seam` (the hook id string directly — e.g. [`lunco_core::session::AUTHORIZE_HOOK`],
/// [`MERGE_SEAM`], a `lunco:driveKernel` id, or any open seam). `deterministic`
/// gates rhai scope reuse: convergent/replicated seams (merge, drive) must be
/// deterministic; the host-only authorization gate need not be.
///
/// This is the projected form of a `LunCoPolicy` USD prim — its
/// `lunco:policy:{seam,entry,source,deterministic}` attributes map one-to-one — so
/// a policy authored in USD rides the journal/sync/persist path with no
/// policy-specific machinery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyDef {
    /// The hook id this policy registers under (open seam-id, no enum).
    pub seam: String,
    /// The rhai entry function name.
    pub entry: String,
    /// The rhai source defining `entry` (+ helpers).
    pub source: String,
    /// Whether the hook is deterministic (fresh rhai scope per invoke).
    pub deterministic: bool,
}

/// The **derived** set of currently-active scripted policies on this peer — the
/// projection cache, NOT an authoritative broadcast source. Rebuilt from the
/// composed `LunCoPolicy` prims by [`project_policies`]; a policy converges across
/// peers because its prim rides the USD journal, so every peer projects the same
/// set. Read it for a "what's active" report.
#[derive(Resource, Default, Clone)]
pub struct ScriptedPolicyRegistry {
    pub policies: Vec<PolicyDef>,
}

/// Compile+register the policy's rhai hook and **activate** it: a policy at
/// [`MERGE_SEAM`] also flips the journal's [`MergeStrategy`] to `Scripted(seam)`;
/// an [`AUTHORIZE_HOOK`](lunco_core::session::AUTHORIZE_HOOK)-seam policy is
/// consulted by [`lunco_core::session::authorize`]; any other seam just registers
/// the hook by id (e.g. a `lunco:driveKernel` id `apply_drive_mix` invokes).
/// Re-registering hot-replaces (idempotent for a stable source).
pub fn apply_policy(def: &PolicyDef, journal: Option<&JournalResource>) -> Result<(), String> {
    // A merge-seam policy with a live journal goes through the journal plane's
    // canonical activation (register hook + flip strategy in one primitive —
    // the single place the `MergeStrategy::Scripted` switch lives). It forces
    // `deterministic` (the merge plane's convergence contract). Any other seam
    // (or no journal yet) just registers the hook by id.
    if def.seam == MERGE_SEAM {
        if let Some(j) = journal {
            return crate::journal_plane::activate_scripted_merge_policy(
                j, &def.seam, &def.entry, &def.source,
            );
        }
    }
    lunco_hooks_rhai::register_rhai_hook(&def.seam, &def.entry, &def.source, def.deterministic)
        .map(|_| ())
}

/// Deactivate a policy whose prim/definition vanished: unregister its hook, and if
/// it drove the journal merge order, reset the strategy to the built-in
/// [`MergeStrategy::Default`]. The counterpart of [`apply_policy`] used by
/// [`project_policies`] when a `LunCoPolicy` prim is removed (closes the
/// "unregister has no production callers" gap).
pub fn retract_policy(seam: &str, journal: Option<&JournalResource>) {
    lunco_hooks::unregister(seam);
    if seam == MERGE_SEAM {
        if let Some(j) = journal {
            crate::journal_plane::use_default_merge_policy(j);
        }
    }
}

/// **Reactive projection** of the full desired policy set (e.g. the composed
/// `LunCoPolicy` prims of a stage) into the live hook registry: register every
/// `desired` policy, **retract any previously-active seam no longer present**, and
/// leave `registry` holding exactly `desired` as the derived activation cache.
///
/// This is the activation half of "policy is a projected USD prim": the caller
/// reads the composed stage into `desired` `PolicyDef`s and calls this; a seam that
/// disappeared (its prim was deleted, or a stronger layer removed the opinion) is
/// unregistered and its merge strategy reset — so activation exactly tracks the
/// authored/composed state, on every peer, with no bespoke broadcast.
pub fn project_policies(
    desired: Vec<PolicyDef>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) {
    // Retract seams that were active but are no longer desired.
    let keep: std::collections::HashSet<&str> = desired.iter().map(|p| p.seam.as_str()).collect();
    for prev in &registry.policies {
        if !keep.contains(prev.seam.as_str()) {
            retract_policy(&prev.seam, journal);
        }
    }
    // (Re-)register every desired policy — hot-replace is idempotent.
    for def in &desired {
        if let Err(e) = apply_policy(def, journal) {
            warn!("[policy] failed to project seam '{}': {e}", def.seam);
        }
    }
    registry.policies = desired;
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_twin_journal::MergeStrategy;

    fn policy(seam: &str, src: &str, deterministic: bool) -> PolicyDef {
        PolicyDef { seam: seam.into(), entry: "cmp".into(), source: src.into(), deterministic }
    }

    #[test]
    fn apply_merge_seam_policy_registers_hook_and_sets_strategy() {
        use lunco_twin_journal::{AuthorId, TwinId};
        let journal = JournalResource::new(TwinId::new("t"), AuthorId::new("me"));
        apply_policy(&policy(MERGE_SEAM, "fn cmp(a, b) { 0 }", true), Some(&journal)).unwrap();
        // Hook registered…
        assert!(lunco_hooks::get(MERGE_SEAM).is_some());
        // …and the journal now linearizes via the scripted strategy at that seam.
        journal.with_read(|j| {
            assert_eq!(*j.merge_strategy(), MergeStrategy::Scripted(MERGE_SEAM.into()));
        });
        lunco_hooks::unregister(MERGE_SEAM);
    }

    #[test]
    fn authorize_seam_policy_is_consulted_by_the_gate() {
        // A policy authored at the gate's seam registers there so `authorize()`
        // consults it — the seam IS the id (open, no enum-pinning).
        apply_policy(
            &policy(lunco_core::session::AUTHORIZE_HOOK, "fn cmp(ctx) { true }", false),
            None,
        )
        .unwrap();
        assert!(lunco_hooks::get(lunco_core::session::AUTHORIZE_HOOK).is_some());
        lunco_hooks::unregister(lunco_core::session::AUTHORIZE_HOOK);
    }

    /// The reactive projector activates every desired policy and **retracts** a
    /// seam that disappeared — the diff that makes activation track the composed
    /// USD stage (a deleted `LunCoPolicy` prim unregisters its hook).
    #[test]
    fn project_policies_registers_desired_and_retracts_vanished() {
        let mut reg = ScriptedPolicyRegistry::default();
        // Round 1: two policies projected + registered.
        project_policies(
            vec![policy("a.seam", "fn cmp(){1}", true), policy("b.seam", "fn cmp(){2}", true)],
            &mut reg,
            None,
        );
        assert!(lunco_hooks::get("a.seam").is_some());
        assert!(lunco_hooks::get("b.seam").is_some());

        // Round 2: `b.seam` vanished from the composed set → retracted (unregistered).
        project_policies(vec![policy("a.seam", "fn cmp(){1}", true)], &mut reg, None);
        assert!(lunco_hooks::get("a.seam").is_some(), "still-desired seam stays active");
        assert!(lunco_hooks::get("b.seam").is_none(), "vanished seam is unregistered");
        assert_eq!(reg.policies.len(), 1, "registry is the derived active-set cache");

        lunco_hooks::unregister("a.seam");
    }
}
