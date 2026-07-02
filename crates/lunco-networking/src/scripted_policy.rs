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

/// Which internal decision point a scripted policy fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyKind {
    /// Convergent journal merge order ([`lunco_twin_journal::MergePolicy`]).
    Merge,
    /// The RBAC authorization gate ([`lunco_core::session::AUTHORIZE_HOOK`]).
    Authorize,
    /// A vehicle drive kernel — a rhai hook named by `lunco:driveKernel` /
    /// [`lunco_core::kernels::DriveMix::kernel`], consumed by `apply_drive_mix`.
    DriveKernel,
}

impl PolicyKind {
    /// Parse a `kind` string from the command / API surface.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "merge" => Some(Self::Merge),
            "authorize" | "authz" | "rbac" => Some(Self::Authorize),
            "drivekernel" | "drive_kernel" | "kernel" => Some(Self::DriveKernel),
            _ => None,
        }
    }

    /// Convergent/replicated policies (merge, drive kernel) must be a pure function
    /// of their inputs and identical on every peer; authorization is host-only and
    /// carries no such contract.
    fn deterministic(self) -> bool {
        !matches!(self, PolicyKind::Authorize)
    }

    /// The hook id this policy MUST register under. Authorization is pinned to the
    /// gate's well-known id (that's the id [`lunco_core::session::authorize`]
    /// consults); the others use the caller-supplied id.
    fn effective_hook_id(self, requested: &str) -> String {
        match self {
            PolicyKind::Authorize => lunco_core::session::AUTHORIZE_HOOK.to_string(),
            _ => requested.to_string(),
        }
    }
}

/// One scripted policy: a rhai `source` whose `entry` function fills the hook of
/// kind `kind`, registered under `hook_id` (ignored for `Authorize`, which pins to
/// the gate's id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyDef {
    pub kind: PolicyKind,
    pub hook_id: String,
    pub entry: String,
    pub source: String,
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

/// Compile+register the policy's rhai hook and **activate** it: a `Merge` policy
/// also flips the journal's [`MergeStrategy`]; `Authorize` registers under the
/// gate's hook id so [`lunco_core::session::authorize`] consults it; a
/// `DriveKernel` just registers the rhai hook by id; `apply_drive_mix` invokes it
/// for any vessel whose `DriveMix.kernel` names that id. Re-registering hot-replaces
/// (idempotent for a stable source).
pub fn apply_policy(def: &PolicyDef, journal: Option<&JournalResource>) -> Result<(), String> {
    let id = def.kind.effective_hook_id(&def.hook_id);
    lunco_hooks_rhai::register_rhai_hook(&id, &def.entry, &def.source, def.kind.deterministic())?;
    if def.kind == PolicyKind::Merge {
        if let Some(j) = journal {
            j.with_write(|jj| jj.set_merge_strategy(MergeStrategy::Scripted(id)));
        }
    }
    Ok(())
}

/// Host entry point: upsert `def` into `registry` (replacing any prior policy it
/// supersedes) and apply it. `Merge`/`Authorize` are singletons (one active each);
/// `DriveKernel`s are keyed by `hook_id` (many vehicles, distinct kernels).
pub fn set_policy(
    registry: &mut ScriptedPolicyRegistry,
    def: PolicyDef,
    journal: Option<&JournalResource>,
) -> Result<(), String> {
    apply_policy(&def, journal)?;
    registry.policies.retain(|p| match def.kind {
        PolicyKind::DriveKernel => !(p.kind == def.kind && p.hook_id == def.hook_id),
        _ => p.kind != def.kind,
    });
    registry.policies.push(def);
    Ok(())
}

/// Client: apply an inbound policy set — register+activate each, and mirror the set
/// into the local registry (so a report / later reconnect is accurate). A single
/// failing policy is logged and skipped, not fatal.
pub fn apply_inbound_policies(
    msg: &ScriptedPolicyMsg,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) {
    for def in &msg.policies {
        if let Err(e) = apply_policy(def, journal) {
            warn!("[policy-plane] failed to apply inbound {:?} policy '{}': {e}", def.kind, def.hook_id);
        }
    }
    registry.policies = msg.policies.clone();
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
    /// `"merge"`, `"authorize"`, or `"drive_kernel"`.
    pub kind: String,
    /// Hook id to register under (ignored for `authorize`, pinned to the gate id).
    pub hook_id: String,
    /// The rhai entry function name (e.g. `"cmp"`).
    pub entry: String,
    /// The rhai source defining `entry` (+ any helpers / consts).
    pub source: String,
}

#[lunco_core::on_command(SetScriptedPolicy)]
fn on_set_scripted_policy(
    trigger: On<SetScriptedPolicy>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    journal: Option<Res<JournalResource>>,
) {
    let cmd = trigger.event();
    let Some(kind) = PolicyKind::parse(&cmd.kind) else {
        warn!("[policy] unknown policy kind '{}' (want merge|authorize|drive_kernel)", cmd.kind);
        return;
    };
    let def = PolicyDef {
        kind,
        hook_id: cmd.hook_id.clone(),
        entry: cmd.entry.clone(),
        source: cmd.source.clone(),
    };
    match set_policy(&mut registry, def, journal.as_deref()) {
        Ok(()) => info!("[policy] activated {kind:?} policy '{}' (broadcasting to peers)", cmd.hook_id),
        Err(e) => warn!("[policy] failed to activate {kind:?} policy: {e}"),
    }
}

lunco_core::register_commands!(on_set_scripted_policy);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parse_is_lenient() {
        assert_eq!(PolicyKind::parse("Merge"), Some(PolicyKind::Merge));
        assert_eq!(PolicyKind::parse("rbac"), Some(PolicyKind::Authorize));
        assert_eq!(PolicyKind::parse(" drive_kernel "), Some(PolicyKind::DriveKernel));
        assert_eq!(PolicyKind::parse("nonsense"), None);
    }

    #[test]
    fn apply_merge_policy_registers_hook_and_sets_strategy() {
        use lunco_twin_journal::{AuthorId, TwinId};
        let journal = JournalResource::new(TwinId::new("t"), AuthorId::new("me"));
        let def = PolicyDef {
            kind: PolicyKind::Merge,
            hook_id: "test.policy.merge".into(),
            entry: "cmp".into(),
            source: "fn cmp(a, b) { 0 }".into(),
        };
        apply_policy(&def, Some(&journal)).unwrap();
        // Hook registered…
        assert!(lunco_hooks::get("test.policy.merge").is_some());
        // …and the journal now linearizes via the scripted strategy.
        journal.with_read(|j| {
            assert_eq!(*j.merge_strategy(), MergeStrategy::Scripted("test.policy.merge".into()));
        });
        lunco_hooks::unregister("test.policy.merge");
    }

    #[test]
    fn authorize_policy_pins_to_the_gate_hook_id() {
        // Whatever hook_id is requested, an authorize policy MUST register under
        // the gate's well-known id (else `authorize()` never consults it).
        let def = PolicyDef {
            kind: PolicyKind::Authorize,
            hook_id: "ignored.custom.id".into(),
            entry: "allow".into(),
            source: "fn allow(ctx) { true }".into(),
        };
        apply_policy(&def, None).unwrap();
        assert!(lunco_hooks::get(lunco_core::session::AUTHORIZE_HOOK).is_some());
        assert!(lunco_hooks::get("ignored.custom.id").is_none());
        lunco_hooks::unregister(lunco_core::session::AUTHORIZE_HOOK);
    }

    #[test]
    fn set_policy_upserts_singletons_and_keys_kernels() {
        let mut reg = ScriptedPolicyRegistry::default();
        let merge = |src: &str| PolicyDef {
            kind: PolicyKind::Merge,
            hook_id: "m".into(),
            entry: "cmp".into(),
            source: src.into(),
        };
        set_policy(&mut reg, merge("fn cmp(a,b){0}"), None).unwrap();
        set_policy(&mut reg, merge("fn cmp(a,b){1}"), None).unwrap();
        // Merge is a singleton — the second replaces the first.
        assert_eq!(reg.policies.iter().filter(|p| p.kind == PolicyKind::Merge).count(), 1);
        assert_eq!(reg.policies[0].source, "fn cmp(a,b){1}");

        // Distinct drive kernels coexist (keyed by hook_id).
        let kernel = |id: &str| PolicyDef {
            kind: PolicyKind::DriveKernel,
            hook_id: id.into(),
            entry: "k".into(),
            source: "fn k(c){#{}}".into(),
        };
        set_policy(&mut reg, kernel("left"), None).unwrap();
        set_policy(&mut reg, kernel("right"), None).unwrap();
        assert_eq!(reg.policies.iter().filter(|p| p.kind == PolicyKind::DriveKernel).count(), 2);

        lunco_hooks::unregister("m");
        lunco_hooks::unregister("left");
        lunco_hooks::unregister("right");
    }

    #[test]
    fn policy_envelope_roundtrips_through_the_wire_codec() {
        // The plane rides the same positional codec as every other envelope, so
        // PolicyDef must be codec-clean (all Strings + a plain enum).
        let policies = vec![PolicyDef {
            kind: PolicyKind::DriveKernel,
            hook_id: "k".into(),
            entry: "k".into(),
            source: "fn k(c){#{}}".into(),
        }];
        let env = SyncEnvelope::ScriptedPolicy(ScriptedPolicyMsg { policies: policies.clone() });
        let bytes = crate::shared::serialize_env(&env).expect("serialize");
        let Some(SyncEnvelope::ScriptedPolicy(back)) = crate::shared::deserialize_env(&bytes) else {
            panic!("did not round-trip to a ScriptedPolicy envelope");
        };
        assert_eq!(back.policies, policies);
    }
}
