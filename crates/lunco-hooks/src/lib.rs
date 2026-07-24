//! Backend-agnostic **internal hook** registry — the extension-point substrate.
//!
//! # Why this exists
//!
//! Some internal decisions want to be *policy*, authored outside Rust: how to
//! order two concurrent edits when merging divergent history, whether a session
//! may perform an action, and (later) physics/render/lifecycle decisions. Baking
//! each into Rust means every new policy is a recompile; scripting each *directly*
//! in rhai hard-wires one language.
//!
//! This crate owns the **one abstraction** every scripting backend implements, so
//! a hook point is defined once (a Rust trait in the crate that owns it, e.g.
//! [`MergePolicy`](../lunco_twin_journal) in `lunco-twin-journal`) and can be
//! *filled* by rhai today, Python/Lua/wasm tomorrow — none of which this crate,
//! or the domain crate, depends on.
//!
//! # Shape (mirrors the proven `lunco-tools` split)
//!
//! - [`HookValue`] — a small, **typed** owned value (NOT JSON) that crosses the
//!   language boundary. Object-safe dispatch needs a concrete value type, so
//!   unlike the read-path [`ValueBuilder`](../lunco_scripting) (generic, monomorphized
//!   per language for zero-copy reflect reads), the hook boundary marshals through
//!   this owned enum. Hook args are small (two journal entries, a session record),
//!   so the one extra conversion hop is irrelevant.
//! - [`ScriptHook`] — the single interface a language backend implements *once*
//!   (`HookValue in → HookValue out`); one impl then services **every** hook.
//! - The global [`register`]/[`invoke`] registry — dependency-free, headless-safe
//!   (works deep inside a pure crate like the journal, with no Bevy/ECS), keyed by
//!   a `HookId` string.
//!
//! # Determinism contract
//!
//! A hook consumed on a **replicated / convergent** path (merge ordering) MUST be
//! a *pure function of its arguments* and **identical on every peer**, or state
//! diverges. Such hooks are registered with [`RegisteredHook::deterministic`] set;
//! the convergent consumer refuses a hook that isn't, and a language binding must
//! give each invocation a fresh state (no cross-call carry). Authorization and
//! other local-only hooks carry no such requirement.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

// ── Neutral value ────────────────────────────────────────────────────────────

/// A language-neutral, owned, **typed** value crossing the hook boundary.
///
/// Deliberately not `serde_json::Value` (per the "no JSON for internal logic"
/// rule) and deliberately owned (an object-safe `dyn ScriptHook` can't be generic
/// over a `ValueBuilder`). Each language binding converts to/from its native value
/// (`HookValue ↔ rhai::Dynamic`, later `↔ PyObject`).
#[derive(Clone, Debug, PartialEq)]
pub enum HookValue {
    /// The unit / nothing value (rhai `()`, Python `None`).
    Unit,
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Str(String),
    /// An ordered array.
    Array(Vec<HookValue>),
    /// A string-keyed map (insertion-ordered; small, so a `Vec` not a `HashMap`).
    Map(Vec<(String, HookValue)>),
}

impl HookValue {
    /// A map value from key/value pairs.
    pub fn map(entries: impl IntoIterator<Item = (impl Into<String>, HookValue)>) -> Self {
        HookValue::Map(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    /// A string value.
    pub fn str(s: impl Into<String>) -> Self {
        HookValue::Str(s.into())
    }
    /// This value as an `i64`, if it is an integer (or a whole float / bool).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            HookValue::Int(i) => Some(*i),
            HookValue::Float(f) => Some(*f as i64),
            HookValue::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }
    /// This value as an `f64`, if numeric.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            HookValue::Float(f) => Some(*f),
            HookValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    /// This value as a `bool`, if boolean (or a nonzero integer).
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            HookValue::Bool(b) => Some(*b),
            HookValue::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
    /// This value as a `&str`, if a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            HookValue::Str(s) => Some(s),
            _ => None,
        }
    }
    /// The value under `key`, if this is a map containing it.
    pub fn get(&self, key: &str) -> Option<&HookValue> {
        match self {
            HookValue::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

// ── The one interface every language implements ──────────────────────────────

/// The result of a hook invocation: the native value it returned, or an error
/// message (a compile/runtime fault in the scripted implementation).
pub type HookResult = Result<HookValue, HookError>;

/// A hook invocation failure — the scripted implementation faulted (raised, threw,
/// or produced the wrong shape). The message is human-facing (surfaced in a log /
/// diagnostic); callers on convergent paths treat it as "policy unavailable".
#[derive(Clone, Debug)]
pub struct HookError(pub String);

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for HookError {}

/// The single, object-safe interface a scripting backend implements to fill *any*
/// hook point. One impl per language (`RhaiHook` in `lunco-hooks-rhai`) services
/// every hook, because everything is [`HookValue`] in and out.
pub trait ScriptHook: Send + Sync + 'static {
    /// Invoke the hook with positional args; return its value or an error.
    fn invoke(&self, args: &[HookValue]) -> HookResult;
}

// ── Registry (mirrors lunco-tools: global, dependency-free, generation-tracked)

/// A registered hook: its id, which backend authored it, whether it is safe for
/// convergent/replicated use (see the crate-level determinism contract), and the
/// callable itself.
pub struct RegisteredHook {
    /// Unique id the hook is invoked by (e.g. `"merge.concurrent_cmp"`).
    pub id: String,
    /// Implementation backend, for discovery: `"rhai"`, `"rust"`, `"python"`, …
    pub backend: String,
    /// `true` ⇒ the hook is a pure function of its args and identical on every
    /// peer, so a convergent consumer (merge) may use it. `false` ⇒ local-only.
    pub deterministic: bool,
    /// The callable.
    pub hook: Arc<dyn ScriptHook>,
}

/// Discovery record for a registered hook (id + backend + determinism), for a
/// `ListHooks` API surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookInfo {
    pub id: String,
    pub backend: String,
    pub deterministic: bool,
}

fn registry() -> &'static RwLock<HashMap<String, Arc<RegisteredHook>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<RegisteredHook>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

fn generation_cell() -> &'static AtomicU64 {
    static G: AtomicU64 = AtomicU64::new(0);
    &G
}

/// Register (or hot-replace) a hook by its [`RegisteredHook::id`]. Bumps the
/// generation. Safe from anywhere (host, command, test, a language binding's
/// refresh). Returns the id, for convenience.
pub fn register(hook: RegisteredHook) -> String {
    let id = hook.id.clone();
    registry()
        .write()
        .unwrap()
        .insert(id.clone(), Arc::new(hook));
    generation_cell().fetch_add(1, Ordering::Relaxed);
    id
}

/// Remove a hook, if present. Bumps the generation.
pub fn unregister(id: &str) {
    if registry().write().unwrap().remove(id).is_some() {
        generation_cell().fetch_add(1, Ordering::Relaxed);
    }
}

/// The registered hook under `id`, if any (clones the `Arc`; cheap).
pub fn get(id: &str) -> Option<Arc<RegisteredHook>> {
    registry().read().unwrap().get(id).cloned()
}

/// Invoke the hook registered under `id`. `None` if no such hook is registered
/// (so the caller can fall back to its built-in behaviour); `Some(Err)` if the
/// hook ran but faulted.
pub fn invoke(id: &str, args: &[HookValue]) -> Option<HookResult> {
    let hook = get(id)?;
    Some(hook.hook.invoke(args))
}

/// Monotonic registry generation — changes on every [`register`]/[`unregister`].
/// A consumer can compare it against a cached value to detect hot-reloads.
pub fn generation() -> u64 {
    generation_cell().load(Ordering::Relaxed)
}

/// Discovery index of every registered hook, sorted by id.
pub fn index() -> Vec<HookInfo> {
    let mut v: Vec<HookInfo> = registry()
        .read()
        .unwrap()
        .values()
        .map(|h| HookInfo {
            id: h.id.clone(),
            backend: h.backend.clone(),
            deterministic: h.deterministic,
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A native (Rust) hook that returns the sum of two int args — proves the
    /// registry works with a non-scripted `ScriptHook` too.
    struct AddHook;
    impl ScriptHook for AddHook {
        fn invoke(&self, args: &[HookValue]) -> HookResult {
            let a = args.first().and_then(HookValue::as_i64).unwrap_or(0);
            let b = args.get(1).and_then(HookValue::as_i64).unwrap_or(0);
            Ok(HookValue::Int(a + b))
        }
    }

    #[test]
    fn register_invoke_and_discover() {
        let gen0 = generation();
        register(RegisteredHook {
            id: "test.add".into(),
            backend: "rust".into(),
            deterministic: true,
            hook: Arc::new(AddHook),
        });
        assert!(generation() > gen0, "register must bump the generation");

        // Absent hook → None (caller falls back to built-in).
        assert!(invoke("test.missing", &[]).is_none());

        // Present hook runs.
        let out = invoke("test.add", &[HookValue::Int(2), HookValue::Int(40)]);
        assert_eq!(out.unwrap().unwrap(), HookValue::Int(42));

        // Discovery reflects the determinism flag.
        let info = index().into_iter().find(|i| i.id == "test.add").unwrap();
        assert_eq!(info.backend, "rust");
        assert!(info.deterministic);

        unregister("test.add");
        assert!(get("test.add").is_none());
    }

    #[test]
    fn hookvalue_accessors() {
        let m = HookValue::map([
            ("lamport", HookValue::Int(7)),
            ("author", HookValue::str("peer-1")),
        ]);
        assert_eq!(m.get("lamport").and_then(HookValue::as_i64), Some(7));
        assert_eq!(m.get("author").and_then(HookValue::as_str), Some("peer-1"));
        assert_eq!(m.get("missing"), None);
    }
}
