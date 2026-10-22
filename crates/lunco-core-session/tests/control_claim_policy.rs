//! Separate process: the hook registry is global and must not race unit tests.
use std::sync::Arc;

use lunco_command_contracts::SessionId;
use lunco_core_session::{
    CONTROL_CLAIM_HOOK, SessionRbac, SessionRegistry, claim_control, release_control,
};
use lunco_hooks::{HookError, HookResult, HookValue, RegisteredHook, ScriptHook};

struct Decision(HookResult);
impl ScriptHook for Decision {
    fn invoke(&self, invocation: &lunco_hooks::HookInvocation<'_>) -> HookResult {
        assert_eq!(
            invocation.args[0].get("target").and_then(HookValue::as_u64),
            Some(42)
        );
        self.0.clone()
    }
}
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        lunco_hooks::unregister(CONTROL_CLAIM_HOOK);
    }
}
fn install(result: HookResult) {
    lunco_hooks::register(RegisteredHook {
        id: CONTROL_CLAIM_HOOK.into(),
        backend: "rust".into(),
        deterministic: false,
        hook: Arc::new(Decision(result)),
    });
}

#[test]
fn claim_policy_reserves_free_endpoints_without_weakening_ownership() {
    let _cleanup = Cleanup;
    let a = SessionId(1);
    let b = SessionId(2);
    let rbac = SessionRbac::default();
    let mut registry = SessionRegistry::default();
    assert!(claim_control(&mut registry, &rbac, a, 42).is_ok());
    release_control(&mut registry, a);

    install(Ok(HookValue::Bool(false)));
    assert!(claim_control(&mut registry, &rbac, a, 42).is_err());
    assert_eq!(registry.owner_of(42), None);

    install(Ok(HookValue::Bool(true)));
    assert!(claim_control(&mut registry, &rbac, a, 42).is_ok());
    assert!(claim_control(&mut registry, &rbac, b, 42).is_err());
    assert_eq!(registry.owner_of(42), Some(a));

    install(Ok(HookValue::Bool(false)));
    assert!(claim_control(&mut registry, &rbac, a, 42).is_err());
    assert_eq!(registry.owner_of(42), Some(a));
    assert_eq!(release_control(&mut registry, a).released, vec![42]);

    install(Err(HookError("broken policy".into())));
    assert!(claim_control(&mut registry, &rbac, a, 42).is_err());
    install(Ok(HookValue::str("invalid decision")));
    assert!(claim_control(&mut registry, &rbac, a, 42).is_err());
    assert_eq!(registry.owner_of(42), None);

    lunco_hooks::unregister(CONTROL_CLAIM_HOOK);
    assert!(claim_control(&mut registry, &rbac, a, 42).is_ok());
}
