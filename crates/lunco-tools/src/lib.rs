//! Backend-agnostic **tool** registry.
//!
//! A *tool* is a named, reusable unit a scenario reaches two ways:
//!
//! - **As a script-call library** — a bundle of callable functions invoked as
//!   `name::fn(...)`. A tool's IMPLEMENTATION is pluggable: rhai source, native
//!   Rust, or (later) any other runtime. The `lunco-tools-rhai` adapter binds
//!   each registered tool into a script engine so a scenario can call it.
//! - **As an engine action** — when a task/program action fires a tool name,
//!   the bevy-aware adapter (`lunco-tools-bevy`) runs it.
//!   That path is bevy-specific (it needs `&mut World`/`Commands`), so it lives
//!   in `lunco-tools-bevy`, NOT here — see [`ExecutableTool`] there.
//!
//! This crate owns only the *abstraction* + the layered registry + discovery,
//! and is deliberately dependency-free so the rhai-binding adapter
//! (`lunco-tools-rhai`) stays slim. The two adapter capabilities —
//! script-binding (rhai) and behaviour-tree execution (bevy) — live in their
//! own crates, each pulling only what it needs.
//!
//! ```ignore
//! // any adapter registers a tool (script-callable; optionally executable)…
//! lunco_tools::register(Arc::new(MyTool));
//! // …lunco-tools-rhai binds it into an engine as `name::fn(...)`,
//! // …lunco-tools-bevy runs it when the action is also executable.
//! ```

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// Lifecycle layer that owns a registered tool.
///
/// Layers are resolved from broad to narrow: the standard library is the base,
/// core mechanisms may replace it, application state may replace core tools,
/// and the active Twin has the narrowest scope. The registry stores every
/// layer separately, so closing a Twin cannot restore a stale snapshot over a
/// newer application or core registration.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum ToolScope {
    /// Shared assets shipped by the engine (`lunco://`).
    Standard,
    /// Always-on engine mechanisms and native substrate tools.
    Core,
    /// Process/application-owned dynamic state.
    Application,
    /// The currently active Twin's authored state.
    Twin(String),
}

impl ToolScope {
    /// Stable API spelling for the owning layer.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Standard => "standard",
            Self::Core => "core",
            Self::Application => "application",
            Self::Twin(_) => "twin",
        }
    }

    /// Stable identity used in diagnostics and API discovery.
    pub fn identity(&self) -> String {
        match self {
            Self::Standard => "standard".into(),
            Self::Core => "core".into(),
            Self::Application => "application".into(),
            Self::Twin(id) => format!("twin:{id}"),
        }
    }
}

/// A named bundle of callable functions, independent of implementation language.
///
/// Implementors live in adapter crates: `RhaiTool` / `NativeRhaiTool` in
/// `lunco-tools-rhai`, a future `PythonTool`, etc. The metadata methods are
/// runtime-neutral (used for discovery here); a runtime adapter downcasts via
/// [`Tool::as_any`], or reads [`Tool::source`], to actually bind the tool.
///
/// This trait is **bevy-free** — it covers discovery + script-binding only. A
/// tool that is ALSO executable as a behaviour-tree action additionally
/// implements `lunco_tools_bevy::ExecutableTool` (which carries `&mut World`).
/// The bevy-free / bevy-aware split keeps `lunco-tools-rhai` slim.
pub trait Tool: Send + Sync + 'static {
    /// Namespace the tool is invoked under (`name::fn(...)`). Unique key.
    fn name(&self) -> &str;
    /// Implementation backend, for discovery: `"rhai"`, `"rust"`, `"python"`, …
    fn backend(&self) -> &str;
    /// Function signatures the tool exposes, as `"fn_name/arity"` strings.
    fn functions(&self) -> Vec<String>;
    /// Textual source, when the tool is source-defined (rhai/python/…); `None`
    /// for native tools. A runtime adapter can bind any source-defined tool
    /// generically by compiling this.
    fn source(&self) -> Option<&str> {
        None
    }
    /// Downcast hook so a runtime adapter can recover a concrete tool type it
    /// knows how to bind (e.g. a native tool carrying a Rust builder closure),
    /// and so `lunco-tools-bevy` can recover an [`ExecutableTool`] supertrait.
    fn as_any(&self) -> &dyn Any;
}

/// Discovery record for one registered tool (the shape exposed over the API).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub backend: String,
    pub functions: Vec<String>,
    /// Layer that currently owns the visible registration.
    pub scope: String,
}

fn registry() -> &'static RwLock<HashMap<(ToolScope, String), Arc<dyn Tool>>> {
    static R: OnceLock<RwLock<HashMap<(ToolScope, String), Arc<dyn Tool>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

fn active_twin() -> &'static RwLock<Option<String>> {
    static T: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    T.get_or_init(|| RwLock::new(None))
}

fn generation_cell() -> &'static AtomicU64 {
    static G: AtomicU64 = AtomicU64::new(0);
    &G
}

/// Register (or hot-replace) a tool by its [`Tool::name`]. Bumps the generation
/// so runtime adapters know to re-bind. Safe from anywhere (host, command, test).
pub fn register(tool: Arc<dyn Tool>) {
    register_scoped(ToolScope::Application, tool);
}

/// Register (or hot-replace) a tool in an explicit lifecycle layer.
pub fn register_scoped(scope: ToolScope, tool: Arc<dyn Tool>) {
    registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((scope, tool.name().to_string()), tool);
    generation_cell().fetch_add(1, Ordering::Relaxed);
}

/// Select the active Twin overlay. `None` removes every Twin layer from the
/// visible registry without touching the stored standard/core/application
/// registrations.
pub fn set_active_twin(twin: Option<String>) {
    let mut active = active_twin()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *active == twin {
        return;
    }
    *active = twin;
    generation_cell().fetch_add(1, Ordering::Relaxed);
}

/// Remove every registration owned by one scope.
pub fn unregister_scope(scope: &ToolScope) -> usize {
    let mut tools = registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = tools.len();
    tools.retain(|(registered_scope, _), _| registered_scope != scope);
    let removed = before - tools.len();
    if removed != 0 {
        generation_cell().fetch_add(1, Ordering::Relaxed);
    }
    removed
}

/// Remove an application-owned tool by name and bump the binding generation.
///
/// Explicit lifecycle owners should use [`unregister_scoped`] instead.
pub fn unregister(name: &str) -> Option<Arc<dyn Tool>> {
    unregister_scoped(&ToolScope::Application, name)
}

/// Remove one tool from an explicit layer.
pub fn unregister_scoped(scope: &ToolScope, name: &str) -> Option<Arc<dyn Tool>> {
    let removed = registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&(scope.clone(), name.to_string()));
    if removed.is_some() {
        generation_cell().fetch_add(1, Ordering::Relaxed);
    }
    removed
}

/// Monotonic registry generation — changes whenever the registry is modified
/// by [`register`] or [`unregister`]. A runtime adapter compares this against
/// its last-bound value to detect added, replaced, or removed tools.
pub fn generation() -> u64 {
    generation_cell().load(Ordering::Relaxed)
}

/// Every visible tool after scope resolution (clones the `Arc`s; cheap). Order
/// unspecified.
pub fn all() -> Vec<Arc<dyn Tool>> {
    visible().into_values().map(|(_, tool)| tool).collect()
}

/// A registered tool by name, if any.
pub fn get(name: &str) -> Option<Arc<dyn Tool>> {
    visible().get(name).map(|(_, tool)| Arc::clone(tool))
}

/// The layer that currently owns the visible tool, if any.
pub fn active_scope(name: &str) -> Option<ToolScope> {
    visible().get(name).map(|(scope, _)| scope.clone())
}

/// Sorted names of every registered tool.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = visible().into_keys().collect();
    v.sort();
    v
}

/// Discovery index (name + backend + function sigs) for every tool, sorted by
/// name — the data behind a `ListTools`/`ListToolLibraries` API query.
pub fn index() -> Vec<ToolInfo> {
    let mut v: Vec<ToolInfo> = visible()
        .into_iter()
        .map(|(name, (scope, t))| ToolInfo {
            name,
            backend: t.backend().to_string(),
            functions: t.functions(),
            scope: scope.identity(),
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Return the visible overlay in resolution order. The map contains one
/// winning registration per name; later layers have narrower ownership.
fn visible() -> HashMap<String, (ToolScope, Arc<dyn Tool>)> {
    let twin = active_twin()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let scopes = [ToolScope::Standard, ToolScope::Core, ToolScope::Application];
    let tools = registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut visible = HashMap::new();
    for scope in scopes {
        for ((registered_scope, name), tool) in tools.iter() {
            if *registered_scope == scope {
                visible.insert(name.clone(), (scope.clone(), Arc::clone(tool)));
            }
        }
    }
    if let Some(twin) = twin {
        let scope = ToolScope::Twin(twin);
        for ((registered_scope, name), tool) in tools.iter() {
            if *registered_scope == scope {
                visible.insert(name.clone(), (scope.clone(), Arc::clone(tool)));
            }
        }
    }
    visible
}

/// The function a tool must expose to become a CLICK TOOL in the editor's Tools
/// palette: `on_click(context)`, called with the structured scene context when
/// the tool is armed and the user clicks the scene. The context includes a
/// `button` field (`"primary"`, `"secondary"`, or `"middle"`) so authored
/// tools can choose their mouse-button policy.
///
/// Declared as a signature rather than a separate registration call so a tool
/// opts into the UI by *being usable from it* — write the handler and the button
/// appears. There is no second list to keep in step, and no way to register a
/// palette entry that then has nothing to run.
pub const UI_CLICK_FN: &str = "on_click/1";

/// Optional companions to [`UI_CLICK_FN`], read the same way. Absent is fine:
/// the palette falls back to the tool's own name and no hint.
pub const UI_LABEL_FN: &str = "ui_label/0";
/// Optional hover text, as `ui_hint/0`.
pub const UI_HINT_FN: &str = "ui_hint/0";

/// Every registered tool that can be armed as a click tool — i.e. exposes
/// [`UI_CLICK_FN`] — sorted by name.
///
/// This is the palette's whole source of truth. Dropping a `.rhai` into
/// `assets/scripting/tools/` with an `on_click(context)` puts a button in the editor,
/// with no Rust involved on either side.
pub fn ui_click_tools() -> Vec<ToolInfo> {
    index()
        .into_iter()
        .filter(|t| t.functions.iter().any(|f| f == UI_CLICK_FN))
        .collect()
}

/// Does tool `name` expose function signature `sig` (e.g. `"ui_hint/0"`)?
pub fn has_function(name: &str, sig: &str) -> bool {
    get(name).is_some_and(|t| t.functions().iter().any(|f| f == sig))
}

/// The textual source of a registered tool, when it is source-defined.
pub fn source(name: &str) -> Option<String> {
    get(name).and_then(|tool| tool.source().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    impl Tool for Dummy {
        fn name(&self) -> &str {
            "dummy"
        }
        fn backend(&self) -> &str {
            "test"
        }
        fn functions(&self) -> Vec<String> {
            vec!["f/1".into()]
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct ScopedDummy(&'static str);
    impl Tool for ScopedDummy {
        fn name(&self) -> &str {
            self.0
        }
        fn backend(&self) -> &str {
            "test"
        }
        fn functions(&self) -> Vec<String> {
            vec!["scope/0".into()]
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn register_then_discover() {
        let gen0 = generation();
        register(Arc::new(Dummy));
        assert!(generation() > gen0, "register must bump the generation");
        assert!(names().contains(&"dummy".to_string()));
        let info = index().into_iter().find(|i| i.name == "dummy").unwrap();
        assert_eq!(info.backend, "test");
        assert_eq!(info.functions, vec!["f/1".to_string()]);
        // native tool → no source
        assert_eq!(source("dummy"), None);
        // downcast hook recovers the concrete type
        assert!(get("dummy").unwrap().as_any().is::<Dummy>());
    }

    #[test]
    fn visible_tool_is_resolved_by_scope_and_twin_shutdown_is_isolated() {
        let name = "scoped_registry_probe";
        register_scoped(ToolScope::Standard, Arc::new(ScopedDummy(name)));
        register_scoped(ToolScope::Core, Arc::new(ScopedDummy(name)));
        register_scoped(ToolScope::Application, Arc::new(ScopedDummy(name)));
        assert_eq!(active_scope(name), Some(ToolScope::Application));

        let twin = ToolScope::Twin("probe".into());
        register_scoped(twin.clone(), Arc::new(ScopedDummy(name)));
        set_active_twin(Some("probe".into()));
        assert_eq!(active_scope(name), Some(twin.clone()));

        unregister_scope(&twin);
        set_active_twin(None);
        assert_eq!(active_scope(name), Some(ToolScope::Application));
        unregister_scope(&ToolScope::Application);
        assert_eq!(active_scope(name), Some(ToolScope::Core));
        unregister_scope(&ToolScope::Core);
        unregister_scope(&ToolScope::Standard);
        assert!(get(name).is_none());
    }
}
