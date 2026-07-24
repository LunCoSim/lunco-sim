//! Scripted-authorization-hook behaviour for [`lunco_core::session::authorize`].
//!
//! In its own test binary (a separate process from the `src` unit tests) because
//! it registers under the **process-global** [`AUTHORIZE_HOOK`] id; doing that in
//! the unit-test binary would race the other `authorize()` tests on parallel
//! threads. The two tests here are serialized via a shared mutex so they never
//! observe each other's global hook.

use std::sync::{Arc, Mutex};

use lunco_core::commands::SessionId;
use lunco_core::session::{
    authorize, authorize_policy, AuthorityRole, CommandPolicyRegistry, ControlPathRegistry,
    SessionRbac, SessionRegistry, UserSession, AUTHORIZE_HOOK,
};
use lunco_hooks::{HookError, HookResult, HookValue, RegisteredHook, ScriptHook};

/// Serializes the two tests, which share the one global `AUTHORIZE_HOOK` slot.
static SERIAL: Mutex<()> = Mutex::new(());

const A: SessionId = SessionId(1);

fn observer_rbac() -> SessionRbac {
    let mut rbac = SessionRbac::default();
    rbac.sessions.insert(
        A.0,
        UserSession {
            session_id: A,
            username: "Observer".to_string(),
            role: AuthorityRole::Observer,
            authenticated: true,
            token: Some("srv-token-a".to_string()),
        },
    );
    rbac
}

/// The hook only FURTHER restricts a request the built-in gate already allowed.
#[test]
fn scripted_hook_tightens_an_open_command() {
    let _guard = SERIAL.lock().unwrap();
    let reg = SessionRegistry::default();
    let pol = CommandPolicyRegistry::default();
    // No blackout declared: these tests are about the hook, not the control path.
    let paths = ControlPathRegistry::default();
    let rbac = observer_rbac();

    // Baseline (no hook): an OPEN command passes.
    assert!(authorize(&reg, &rbac, &pol, &paths, A, "OpenCmd", None).is_ok());

    // A hook that denies exactly "SecretCmd", allows everything else.
    struct DenySecret;
    impl ScriptHook for DenySecret {
        fn invoke(&self, args: &[HookValue]) -> HookResult {
            let cap = args[0]
                .get("capability")
                .and_then(HookValue::as_str)
                .unwrap_or("");
            Ok(HookValue::Bool(cap != "SecretCmd"))
        }
    }
    lunco_hooks::register(RegisteredHook {
        id: AUTHORIZE_HOOK.into(),
        backend: "rust".into(),
        deterministic: false,
        hook: Arc::new(DenySecret),
    });

    // Tightened: the OPEN "SecretCmd" is now denied…
    assert!(authorize(&reg, &rbac, &pol, &paths, A, "SecretCmd", None).is_err());
    // …other commands the hook allows still pass.
    assert!(authorize(&reg, &rbac, &pol, &paths, A, "OpenCmd", None).is_ok());

    // Removing the hook restores the exact pre-hook behaviour.
    lunco_hooks::unregister(AUTHORIZE_HOOK);
    assert!(authorize(&reg, &rbac, &pol, &paths, A, "SecretCmd", None).is_ok());
}

/// A hook that faults denies (fail **closed**) — a broken security policy must
/// never silently wave requests through.
#[test]
fn faulting_hook_fails_closed() {
    let _guard = SERIAL.lock().unwrap();
    let reg = SessionRegistry::default();
    let pol = CommandPolicyRegistry::default();
    // No blackout declared: these tests are about the hook, not the control path.
    let paths = ControlPathRegistry::default();
    let rbac = observer_rbac();

    struct Boom;
    impl ScriptHook for Boom {
        fn invoke(&self, _args: &[HookValue]) -> HookResult {
            Err(HookError("policy crashed".into()))
        }
    }
    lunco_hooks::register(RegisteredHook {
        id: AUTHORIZE_HOOK.into(),
        backend: "rust".into(),
        deterministic: false,
        hook: Arc::new(Boom),
    });

    assert!(
        authorize(&reg, &rbac, &pol, &paths, A, "OpenCmd", None).is_err(),
        "a faulting authorization hook must fail closed",
    );

    lunco_hooks::unregister(AUTHORIZE_HOOK);
    assert!(authorize(&reg, &rbac, &pol, &paths, A, "OpenCmd", None).is_ok());
}
