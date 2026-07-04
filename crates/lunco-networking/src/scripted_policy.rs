//! **Scripted-policy plane** — distribute + activate rhai policies host→client.
//!
//! The hook substrate ([`lunco_hooks`]) lets internal decisions be authored in
//! rhai: the convergent **merge** order ([`lunco_twin_journal::MergePolicy`]), the
//! **authorization** gate ([`lunco_core::session::AUTHORIZE_HOOK`]), and per-vehicle
//! **drive kernels** (a `lunco:driveKernel` hook id in [`lunco_core::kernels::DriveMix`];
//! `apply_drive_mix` falls back to the hook when the name isn't a built-in kernel).
//! But a scripted
//! policy is only correct if **every peer runs the identical one** — most sharply
//! for the merge policy, whose determinism contract is that all peers linearize
//! history the same way or their scenes diverge.
//!
//! This plane makes that true. Policies are host-authoritative declarative state
//! ([`ScriptedPolicyRegistry`]): set on the host via the [`SetScriptedPolicy`]
//! command (the activation surface — HTTP API / MCP / scripting), then **broadcast
//! to every client on connect and on change** (the same shape as the ownership /
//! profile / journal broadcasts). A client applies each policy — compiling the
//! rhai source and registering the hook — so the whole session converges on one
//! policy set, late joiners included.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use lunco_core::{NetworkRole, SyncChannel};
use lunco_doc_bevy::JournalResource;
use lunco_twin_journal::MergeStrategy;

use crate::sync::{SyncEnvelope, SyncOutbox};

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
/// This is the projected form of a `LuncoPolicy` USD prim — its
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

/// The active scripted policies on THIS peer. Host-authoritative: set via
/// [`SetScriptedPolicy`], broadcast to clients on connect + on change, so every
/// peer converges on the identical set.
#[derive(Resource, Default, Clone)]
pub struct ScriptedPolicyRegistry {
    pub policies: Vec<PolicyDef>,
}

/// Host → client: the full active policy set (small; sent on connect + on change).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptedPolicyMsg {
    pub policies: Vec<PolicyDef>,
}

/// Compile+register the policy's rhai hook and **activate** it: a policy at
/// [`MERGE_SEAM`] also flips the journal's [`MergeStrategy`] to `Scripted(seam)`;
/// an [`AUTHORIZE_HOOK`](lunco_core::session::AUTHORIZE_HOOK)-seam policy is
/// consulted by [`lunco_core::session::authorize`]; any other seam just registers
/// the hook by id (e.g. a `lunco:driveKernel` id `apply_drive_mix` invokes).
/// Re-registering hot-replaces (idempotent for a stable source).
pub fn apply_policy(def: &PolicyDef, journal: Option<&JournalResource>) -> Result<(), String> {
    lunco_hooks_rhai::register_rhai_hook(&def.seam, &def.entry, &def.source, def.deterministic)?;
    if def.seam == MERGE_SEAM {
        if let Some(j) = journal {
            j.with_write(|jj| jj.set_merge_strategy(MergeStrategy::Scripted(def.seam.clone())));
        }
    }
    Ok(())
}

/// Deactivate a policy whose prim/definition vanished: unregister its hook, and if
/// it drove the journal merge order, reset the strategy to the built-in
/// [`MergeStrategy::Default`]. The counterpart of [`apply_policy`] used by
/// [`project_policies`] when a `LuncoPolicy` prim is removed (closes the
/// "unregister has no production callers" gap).
pub fn retract_policy(seam: &str, journal: Option<&JournalResource>) {
    lunco_hooks::unregister(seam);
    if seam == MERGE_SEAM {
        if let Some(j) = journal {
            j.with_write(|jj| jj.set_merge_strategy(MergeStrategy::Default));
        }
    }
}

/// Host entry point: upsert `def` into `registry` (replacing any prior policy at
/// the same seam) and apply it. Seams are singletons — one active hook per id.
pub fn set_policy(
    registry: &mut ScriptedPolicyRegistry,
    def: PolicyDef,
    journal: Option<&JournalResource>,
) -> Result<(), String> {
    apply_policy(&def, journal)?;
    registry.policies.retain(|p| p.seam != def.seam);
    registry.policies.push(def);
    Ok(())
}

/// **Reactive projection** of the full desired policy set (e.g. the composed
/// `LuncoPolicy` prims of a stage) into the live hook registry: register every
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

/// Client: apply an inbound policy set — register+activate each, and mirror the set
/// into the local registry (so a report / later reconnect is accurate). A single
/// failing policy is logged and skipped, not fatal.
pub fn apply_inbound_policies(
    msg: &ScriptedPolicyMsg,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) {
    project_policies(msg.policies.clone(), registry, journal);
}

/// Host: when the policy registry changes, broadcast the full set to all peers
/// (reliable `BulkData` lane). Clients converge on the identical policies; late
/// joiners get the set on connect (`on_server_connected`). No-op off-host.
pub fn broadcast_scripted_policies(
    role: Res<NetworkRole>,
    registry: Res<ScriptedPolicyRegistry>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if !role.is_host() || !registry.is_changed() {
        return;
    }
    outbox.0.push((
        SyncChannel::BulkData,
        SyncEnvelope::ScriptedPolicy(ScriptedPolicyMsg { policies: registry.policies.clone() }),
    ));
}

/// Activate a scripted policy (merge / authorization / drive-kernel) authored in
/// rhai — the canonical activation surface (HTTP API / MCP / scripting).
///
/// `kind` ∈ `{"merge", "authorize", "drive_kernel"}`. On the host it compiles +
/// registers the hook, activates it, and records it in [`ScriptedPolicyRegistry`],
/// which then broadcasts to every peer (connected + late joiners) so all run the
/// identical policy — the determinism contract for the merge policy. Distribute
/// the SAME `source` you author here to peers via this command (the plane does it
/// for you); do not hand-register divergent scripts per peer.
#[lunco_core::Command(default)]
pub struct SetScriptedPolicy {
    /// The seam (hook id) to register under — an open id, e.g.
    /// `"journal.merge.order"` ([`MERGE_SEAM`]), `"rbac.authorize"`, or a
    /// `lunco:driveKernel` id.
    pub seam: String,
    /// The rhai entry function name (e.g. `"cmp"`).
    pub entry: String,
    /// The rhai source defining `entry` (+ any helpers / consts).
    pub source: String,
    /// Whether the hook is deterministic (fresh rhai scope per invoke). Convergent
    /// seams (merge, drive) must be `true`; the host-only authorize gate may be
    /// `false`.
    pub deterministic: bool,
}

#[lunco_core::on_command(SetScriptedPolicy)]
fn on_set_scripted_policy(
    trigger: On<SetScriptedPolicy>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    journal: Option<Res<JournalResource>>,
) {
    let cmd = trigger.event();
    let def = PolicyDef {
        seam: cmd.seam.clone(),
        entry: cmd.entry.clone(),
        source: cmd.source.clone(),
        deterministic: cmd.deterministic,
    };
    let seam = def.seam.clone();
    match set_policy(&mut registry, def, journal.as_deref()) {
        Ok(()) => info!("[policy] activated policy at seam '{seam}' (broadcasting to peers)"),
        Err(e) => warn!("[policy] failed to activate policy at seam '{seam}': {e}"),
    }
}

lunco_core::register_commands!(on_set_scripted_policy);

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn set_policy_upserts_by_seam() {
        let mut reg = ScriptedPolicyRegistry::default();
        set_policy(&mut reg, policy("m", "fn cmp(a,b){0}", true), None).unwrap();
        set_policy(&mut reg, policy("m", "fn cmp(a,b){1}", true), None).unwrap();
        // A seam is a singleton — the second replaces the first.
        assert_eq!(reg.policies.iter().filter(|p| p.seam == "m").count(), 1);
        assert_eq!(reg.policies[0].source, "fn cmp(a,b){1}");

        // Distinct seams coexist.
        set_policy(&mut reg, policy("left", "fn cmp(c){#{}}", true), None).unwrap();
        set_policy(&mut reg, policy("right", "fn cmp(c){#{}}", true), None).unwrap();
        assert_eq!(reg.policies.len(), 3);

        lunco_hooks::unregister("m");
        lunco_hooks::unregister("left");
        lunco_hooks::unregister("right");
    }

    /// The reactive projector activates every desired policy and **retracts** a
    /// seam that disappeared — the diff that makes activation track the composed
    /// USD stage (a deleted `LuncoPolicy` prim unregisters its hook).
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

    #[test]
    fn policy_envelope_roundtrips_through_the_wire_codec() {
        // The plane rides the same positional codec as every other envelope, so
        // PolicyDef must be codec-clean (all Strings + a bool).
        let policies = vec![policy("k", "fn cmp(c){#{}}", true)];
        let env = SyncEnvelope::ScriptedPolicy(ScriptedPolicyMsg { policies: policies.clone() });
        let bytes = crate::shared::serialize_env(&env).expect("serialize");
        let Some(SyncEnvelope::ScriptedPolicy(back)) = crate::shared::deserialize_env(&bytes) else {
            panic!("did not round-trip to a ScriptedPolicy envelope");
        };
        assert_eq!(back.policies, policies);
    }
}
