//! **Scripted-policy activation** — compile rhai policies into the hook registry.
//!
//! The hook substrate ([`lunco_hooks`]) lets internal decisions be authored in
//! rhai: the convergent **merge** order ([`MERGE_SEAM`]), the **authorization**
//! gate, authored actuation policies, and any application-defined seam such as a
//! generated Modelica synthesizer. A policy is an ordinary projected definition;
//! this module owns activation for both standalone and networked applications.
//!
//! Distribution remains outside this module. In the networked app, the
//! definition is carried by a journaled USD policy prim and every peer projects
//! the same composed set. The registry below is only the derived active cache.

use bevy::prelude::*;
use lunco_doc_bevy::JournalResource;
use lunco_twin_journal::MergeStrategy;
use serde::{Deserialize, Serialize};

/// The reserved policy seam that drives the journal's convergent merge order.
pub const MERGE_SEAM: &str = "journal.merge.order";

/// One scripted policy: a rhai `source` whose `entry` function fills the hook at
/// `seam`. The seam is open so future decisions do not require a Rust enum or
/// branch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyDef {
    /// The hook id this policy registers under.
    pub seam: String,
    /// The rhai entry function name.
    pub entry: String,
    /// The rhai source defining `entry` and its helpers.
    pub source: String,
    /// Whether the hook is deterministic (fresh rhai scope per invoke).
    pub deterministic: bool,
}

/// The derived set of active scripted policies on this process.
#[derive(Resource, Default, Clone)]
pub struct ScriptedPolicyRegistry {
    /// The definitions currently projected into the hook registry.
    pub policies: Vec<PolicyDef>,
}

/// Compile and register a policy, activating the journal merge strategy when
/// the reserved merge seam is used.
pub fn apply_policy(def: &PolicyDef, journal: Option<&JournalResource>) -> Result<(), String> {
    if def.seam == MERGE_SEAM {
        if let Some(journal) = journal {
            return activate_scripted_merge_policy(journal, &def.seam, &def.entry, &def.source);
        }
    }
    lunco_hooks_rhai::register_rhai_hook(&def.seam, &def.entry, &def.source, def.deterministic)
        .map(|_| ())
}

/// Deactivate a policy whose definition vanished and restore the default
/// journal order when it owned the merge seam.
pub fn retract_policy(seam: &str, journal: Option<&JournalResource>) {
    lunco_hooks::unregister(seam);
    if seam == MERGE_SEAM {
        if let Some(journal) = journal {
            use_default_merge_policy(journal);
        }
    }
}

/// Project the complete desired policy set into the live hook registry.
/// Re-registration hot-replaces changed source, while vanished seams are
/// unregistered so activation exactly follows the composed USD state.
pub fn project_policies(
    desired: Vec<PolicyDef>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) {
    let keep: std::collections::HashSet<&str> = desired.iter().map(|p| p.seam.as_str()).collect();
    for previous in &registry.policies {
        if !keep.contains(previous.seam.as_str()) {
            retract_policy(&previous.seam, journal);
        }
    }
    for def in &desired {
        if let Err(error) = apply_policy(def, journal) {
            warn!("[policy] failed to project seam '{}': {error}", def.seam);
        }
    }
    registry.policies = desired;
}

/// Activate a deterministic, convergent rhai merge policy and switch the
/// journal to it only after compilation succeeds.
pub fn activate_scripted_merge_policy(
    journal: &JournalResource,
    hook_id: &str,
    entry: &str,
    source: &str,
) -> Result<(), String> {
    lunco_hooks_rhai::register_rhai_hook(hook_id, entry, source, true)?;
    journal.with_write(|journal| {
        journal.set_merge_strategy(MergeStrategy::Scripted(hook_id.to_string()))
    });
    Ok(())
}

/// Revert a journal to its built-in convergent `(lamport, author)` order.
pub fn use_default_merge_policy(journal: &JournalResource) {
    journal.with_write(|journal| journal.set_merge_strategy(MergeStrategy::Default));
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_twin_journal::MergeStrategy;

    fn policy(seam: &str, source: &str, deterministic: bool) -> PolicyDef {
        PolicyDef {
            seam: seam.into(),
            entry: "cmp".into(),
            source: source.into(),
            deterministic,
        }
    }

    #[test]
    fn project_policies_registers_and_retracts_the_active_set() {
        let mut registry = ScriptedPolicyRegistry::default();
        project_policies(
            vec![
                policy("policy.a", "fn cmp(){1}", true),
                policy("policy.b", "fn cmp(){2}", true),
            ],
            &mut registry,
            None,
        );
        assert!(lunco_hooks::get("policy.a").is_some());
        assert!(lunco_hooks::get("policy.b").is_some());

        project_policies(
            vec![policy("policy.a", "fn cmp(){1}", true)],
            &mut registry,
            None,
        );
        assert!(lunco_hooks::get("policy.a").is_some());
        assert!(lunco_hooks::get("policy.b").is_none());
        lunco_hooks::unregister("policy.a");
    }

    #[test]
    fn merge_policy_switches_and_reverts_the_journal() {
        use lunco_twin_journal::{AuthorId, TwinId};
        let journal = JournalResource::new(TwinId::new("policy"), AuthorId::new("me"));
        apply_policy(&policy(MERGE_SEAM, "fn cmp(a,b){0}", true), Some(&journal)).unwrap();
        journal.with_read(|journal| {
            assert_eq!(
                *journal.merge_strategy(),
                MergeStrategy::Scripted(MERGE_SEAM.into())
            );
        });
        retract_policy(MERGE_SEAM, Some(&journal));
        journal.with_read(|journal| assert_eq!(*journal.merge_strategy(), MergeStrategy::Default));
    }
}
