//! Twin-replaceable policy for reusing content-addressed build artifacts.
//!
//! Rust remains authoritative for cache identity and freshness. The Rhai hook
//! may reuse an exact, validated hit or request a rebuild; it cannot turn a
//! stale or missing artifact into a hit. Processors publish rebuilt files
//! atomically, while Modelica compilation keeps its own source-content key.

use lunco_hooks::HookValue;

/// Hook id used by processed asset and compiled Modelica artifact owners.
pub const ARTIFACT_CACHE_POLICY_HOOK: &str = "artifact.cache.policy";

lunco_hooks::declare_hook! {
    id: ARTIFACT_CACHE_POLICY_HOOK,
    owner: "lunco-assets-core",
    description: "Choose whether to reuse an exact content-addressed artifact hit or rebuild it.",
    signature: [facts: Map],
    output: Map,
    deterministic: true,
    required: false,
    installable: true,
}

/// The policy can only choose whether an already validated exact hit is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactCacheDecision {
    /// Reuse an existing artifact only when Rust has verified its content key.
    Reuse,
    /// Skip a matching cache entry and rebuild it through the owning subsystem.
    Rebuild,
}

/// Invoke the optional policy and enforce the freshness boundary in Rust.
///
/// `facts` should contain `artifact_kind`, `scope`, `logical_id`, `cache_key`,
/// and `cache_hit`. Logical identifiers are portable URIs or relative artifact
/// names; native filesystem cache paths and source payloads are never exposed.
/// A forced rebuild bypasses policy so an explicit rebuild action is final.
pub fn artifact_cache_decision(
    artifact_kind: &str,
    scope: &str,
    logical_id: &str,
    cache_key: &str,
    cache_hit: bool,
    force_rebuild: bool,
) -> Result<ArtifactCacheDecision, String> {
    if force_rebuild {
        return Ok(ArtifactCacheDecision::Rebuild);
    }

    let facts = HookValue::map([
        ("artifact_kind", HookValue::str(artifact_kind)),
        ("scope", HookValue::str(scope)),
        ("logical_id", HookValue::str(logical_id)),
        ("cache_key", HookValue::str(cache_key)),
        ("cache_hit", HookValue::Bool(cache_hit)),
    ]);
    let Some(result) = lunco_hooks::invoke_unclassified(ARTIFACT_CACHE_POLICY_HOOK, &[facts])
    else {
        return Ok(default_decision(cache_hit));
    };
    let value = result.map_err(|error| format!("artifact cache policy failed: {error}"))?;
    decision_from_value(value, cache_hit)
}

fn default_decision(cache_hit: bool) -> ArtifactCacheDecision {
    if cache_hit {
        ArtifactCacheDecision::Reuse
    } else {
        ArtifactCacheDecision::Rebuild
    }
}

fn decision_from_value(value: HookValue, cache_hit: bool) -> Result<ArtifactCacheDecision, String> {
    let HookValue::Map(fields) = value else {
        return Err("artifact cache policy must return a map".into());
    };
    for (name, _) in &fields {
        if !["action", "reason"].contains(&name.as_str()) {
            return Err(format!(
                "artifact cache policy returned unknown field `{name}`"
            ));
        }
    }
    let action = fields
        .iter()
        .find(|(name, _)| name == "action")
        .and_then(|(_, value)| match value {
            HookValue::Str(value) => Some(value.as_str()),
            _ => None,
        })
        .ok_or_else(|| "artifact cache policy requires string field `action`".to_owned())?;
    if fields.iter().filter(|(name, _)| name == "action").count() != 1 {
        return Err("artifact cache policy must return `action` exactly once".into());
    }
    if fields.iter().filter(|(name, _)| name == "reason").count() > 1 {
        return Err("artifact cache policy returned `reason` more than once".into());
    }
    if let Some((_, value)) = fields.iter().find(|(name, _)| name == "reason")
        && !matches!(value, HookValue::Str(_))
    {
        return Err("artifact cache policy field `reason` must be a string".into());
    }

    let requested = match action {
        "reuse" => ArtifactCacheDecision::Reuse,
        "rebuild" => ArtifactCacheDecision::Rebuild,
        other => {
            return Err(format!(
                "artifact cache policy action must be `reuse` or `rebuild`, got `{other}`"
            ));
        }
    };
    // Policy is not an identity override: a stale/missing cache remains a miss.
    Ok(if cache_hit {
        requested
    } else {
        ArtifactCacheDecision::Rebuild
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_cannot_reuse_a_missing_or_stale_artifact() {
        assert_eq!(
            decision_from_value(HookValue::map([("action", HookValue::str("reuse"))]), false,)
                .unwrap(),
            ArtifactCacheDecision::Rebuild
        );
    }

    #[test]
    fn policy_can_rebuild_an_exact_hit() {
        assert_eq!(
            decision_from_value(
                HookValue::map([("action", HookValue::str("rebuild"))]),
                true,
            )
            .unwrap(),
            ArtifactCacheDecision::Rebuild
        );
    }

    #[test]
    fn malformed_policy_output_fails_closed() {
        assert!(
            decision_from_value(HookValue::map([("action", HookValue::str("skip"))]), true,)
                .is_err()
        );
    }
}
