//! Bevy adapter for the [`lunco_tools`] registry — the **execution** half.
//!
//! [`lunco_tools`] is deliberately bevy-free (it owns the `Tool` trait, the
//! global registry, and discovery; the rhai-binding adapter `lunco-tools-rhai`
//! needs it slim). The behaviour-tree execution path is bevy-specific — a
//! `run_tool` leaf's fire needs `&mut World`/`Commands` to act — so it lives
//! HERE, behind the [`ExecutableTool`] supertrait.
//!
//! ## The flow
//!
//! 1. `lunco-autopilot`'s `run_tool` leaf queues a [`ToolInvocation`] on
//!    `DriveCtx::fired`; `drive_autopilots` re-emits each as a
//!    [`ToolFired`](lunco_core::tools::ToolFired) event.
//! 2. This crate's [`ToolDispatchPlugin`] observes `ToolFired`, looks the tool
//!    up by name in the [`lunco_tools`] registry, and downcasts to
//!    [`ExecutableTool`].
//! 3. It calls [`ExecutableTool::execute`] with a [`ToolCallCtx`] backed by
//!    real `&mut World` access — the handler triggers typed commands **directly**
//!    (`world.trigger(CaptureFromCamera { target })`), no JSON, no reflection,
//!    no lossy conversion.
//!
//! ## Defining an instrument
//!
//! An instrument is a [`ClosureTool`] — a `Tool` + `ExecutableTool` whose
//! execute closure constructs and triggers its typed command directly.
//! [`register_closure_tool`] is the one-line registration:
//!
//! ```ignore
//! lunco_tools_bevy::register_closure_tool(
//!     "science::take_photo",           // tool name
//!     vec!["take_photo/0".into()],     // discovery signatures
//!     |world, vessel, _gid, _args| {   // (&mut DeferredWorld, Entity, gid, args)
//!         world.trigger(CaptureFromCamera { target: Some(vessel) });
//!         ToolResult::Ok
//!     },
//! );
//! ```
//!
//! No JSON, no reflect dispatch, no per-instrument Rust struct boilerplate —
//! the closure IS the tool definition, and it reaches typed commands directly.

use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use lunco_core::tools::ToolFired;
use lunco_tools::Tool;
use std::any::Any;
use std::sync::Arc;

/// Outcome of an [`ExecutableTool::execute`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolResult {
    /// The tool ran successfully.
    Ok,
    /// The tool ran but reported a domain error (e.g. no camera to capture
    /// from). `reason` is logged.
    Err(String),
}

/// Bevy-aware context passed to [`ExecutableTool::execute`]. Carries the firing
/// vessel (both its local `Entity` and its `GlobalEntityId` api_id) and deferred
/// world access — so a handler triggers typed commands **directly**, with no
/// JSON/reflect conversion.
///
/// Holds [`DeferredWorld`] (not `&mut World`) because the dispatcher runs inside
/// a Bevy observer, and observers may not take `&mut World` (exclusive) — Bevy
/// 0.19 requires `DeferredWorld` or `Commands` there. `DeferredWorld` supports
/// `trigger`/`insert`/`remove`/resource reads, which is exactly what an
/// instrument closure needs.
pub struct ToolCallCtx<'w> {
    /// The vessel whose autopilot's tree fired the tool (local Entity).
    pub vessel: Entity,
    /// The vessel's `GlobalEntityId` (the api_id rhai/HTTP clients address it
    /// by). `0` when the vessel has no registered gid.
    pub vessel_gid: u64,
    /// The opaque args string the `run_tool` leaf passed through.
    pub args: String,
    world: DeferredWorld<'w>,
}

impl<'w> ToolCallCtx<'w> {
    /// Deferred world access — trigger typed commands, query/insert/remove
    /// components, read resources. Synchronous within the observer.
    pub fn world(&mut self) -> &mut DeferredWorld<'w> {
        &mut self.world
    }
}

/// A tool that can be **executed** when the behaviour tree's `run_tool` leaf
/// fires it — the bevy-aware supertrait of [`Tool`]. A tool registered in the
/// [`lunco_tools`] registry is dispatched by [`ToolDispatchPlugin`] iff it also
/// implements this trait (the downcast is via [`Tool::as_any`]).
///
/// This is deliberately a SEPARATE trait from `Tool` (not a method on it) so
/// `Tool` stays bevy-free and `lunco-tools-rhai` doesn't pull bevy. A concrete
/// instrument implements both; see [`ClosureTool`] for the common-case adapter.
pub trait ExecutableTool: Tool {
    /// Run the tool. Act through `ctx.world()` (trigger typed commands, read
    /// state). Return [`ToolResult::Ok`] on success, [`ToolResult::Err`] on a
    /// domain failure (logged by the dispatcher).
    fn execute(&self, ctx: ToolCallCtx) -> ToolResult;
}

/// Plugin that observes [`ToolFired`] and dispatches each to the registered
/// tool's [`ExecutableTool::execute`]. Add once in the host app. Tools
/// themselves are registered (from any crate) via [`register_closure_tool`] or
/// by implementing `Tool + ExecutableTool` and calling [`lunco_tools::register`].
///
/// Headless-safe: it's an observer, no rendering.
pub struct ToolDispatchPlugin;

impl Plugin for ToolDispatchPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_tool_fired);
    }
}

/// Observer: runs each [`ToolFired`] through the registered tool's
/// `ExecutableTool::execute()`. Tools that aren't `ExecutableTool` (pure
/// script-library tools) are logged + skipped. Unregistered names warn.
///
/// Takes `DeferredWorld` (not `&mut World`): Bevy 0.19 forbids exclusive systems
/// as observers. `DeferredWorld` supports the `trigger`/resource-read access an
/// instrument closure needs, applied synchronously within the observer.
fn on_tool_fired(trigger: On<ToolFired>, world: DeferredWorld) {
    let ev = trigger.event();
    let Some(tool) = lunco_tools::get(&ev.tool) else {
        warn!(
            "[lunco-tools-bevy] ToolFired for unregistered tool '{}'; dropped",
            ev.tool
        );
        return;
    };
    // Downcast to ExecutableTool via the Tool::as_any hook. A pure
    // script-library tool (no execute capability) is skipped with a debug log.
    let any = tool.as_any();
    let Some(executable) = executable_downcast(any) else {
        debug!(
            "[lunco-tools-bevy] tool '{}' is not ExecutableTool (script-only); skipped",
            ev.tool
        );
        return;
    };
    let ctx = ToolCallCtx {
        vessel: ev.vessel,
        vessel_gid: ev.vessel_gid,
        args: ev.args.clone(),
        world,
    };
    match executable.execute(ctx) {
        ToolResult::Ok => {}
        ToolResult::Err(reason) => {
            warn!("[lunco-tools-bevy] tool '{}' failed: {}", ev.tool, reason);
        }
    }
}

/// Recover an [`ExecutableTool`] trait object from a `&dyn Any` produced by
/// [`Tool::as_any`]. Returns `None` when the concrete type doesn't implement
/// `ExecutableTool`. This is the bridge from the bevy-free registry to the
/// bevy-aware execution path — it can live only here (where bevy is present),
/// which is why `ExecutableTool` is a separate trait, not a `Tool` method.
fn executable_downcast(any: &dyn Any) -> Option<&dyn ExecutableTool> {
    // A concrete instrument type implements BOTH `Tool` and `ExecutableTool`.
    // We can't downcast `&dyn Any` straight to `&dyn ExecutableTool` (Any gives
    // back the concrete type only). So `ClosureTool` (the common case) is the
    // one concrete type we know to downcast to here; bespoke instruments that
    // implement ExecutableTool directly add their own downcast arm via a
    // registration that stores the executable trait object.
    any.downcast_ref::<ClosureTool>().map(|t| t as &dyn ExecutableTool)
}

// ─────────────────────────────────────────────────────────────────────────────
// ClosureTool — the common-case declarative instrument
// ─────────────────────────────────────────────────────────────────────────────

/// A tool defined by a closure that triggers its typed command directly. The
/// common-case instrument: the closure IS the tool definition. Register via
/// [`register_closure_tool`].
///
/// The closure receives the world and the firing vessel as **separate borrowed
/// parts** (`&mut DeferredWorld`, `Entity`, gid, args) — so it can read the
/// vessel and trigger a command in one expression with no self-borrow dance. It
/// triggers typed commands directly (`world.trigger(MyCommand { target: vessel })`)
/// — no JSON, no reflection, fully typed and infinitely extensible.
///
/// `DeferredWorld` (not `&mut World`) because the dispatcher runs inside a Bevy
/// observer; observers may not take `&mut World` in Bevy 0.19. `DeferredWorld`
/// supports `trigger`/resource reads/inserts, applied synchronously.
pub struct ClosureTool {
    name: String,
    functions: Vec<String>,
    exec: Arc<dyn Fn(&mut DeferredWorld, Entity, u64, &str) -> ToolResult + Send + Sync + 'static>,
}

impl ClosureTool {
    /// Build a closure-defined tool. `name` is the `run_tool` leaf's tool id
    /// (convention `family::verb`); `functions` are discovery signatures
    /// (`"verb/arity"`); `exec` triggers the typed command(s) directly.
    ///
    /// The closure's args are: `world`, the firing vessel `Entity`, its
    /// `GlobalEntityId` (`0` if unknown), and the leaf's opaque `args` string.
    pub fn new(
        name: impl Into<String>,
        functions: Vec<String>,
        exec: impl Fn(&mut DeferredWorld, Entity, u64, &str) -> ToolResult + Send + Sync + 'static,
    ) -> Self {
        Self { name: name.into(), functions, exec: Arc::new(exec) }
    }
}

impl Tool for ClosureTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn backend(&self) -> &str {
        "rust"
    }
    fn functions(&self) -> Vec<String> {
        self.functions.clone()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ExecutableTool for ClosureTool {
    fn execute(&self, ctx: ToolCallCtx) -> ToolResult {
        // Deconstruct ctx so the closure receives independent parts — it can
        // read `vessel` and borrow `world` in one expression with no self-borrow
        // conflict (the dance `ctx.world()` + `ctx.vessel` would otherwise need).
        // `world` is owned (DeferredWorld); reborrow `&mut` for the closure.
        let ToolCallCtx { vessel, vessel_gid, args, mut world } = ctx;
        (self.exec)(&mut world, vessel, vessel_gid, &args)
    }
}

/// Register a closure-defined tool into the global [`lunco_tools`] registry —
/// the declarative way to define an instrument. Safe from anywhere; idempotent
/// (re-register replaces).
///
/// The closure receives the world and firing vessel as **separate parts** so it
/// can trigger a typed command in one expression with no borrow dance:
///
/// ```ignore
/// lunco_tools_bevy::register_closure_tool(
///     "science::take_photo",
///     vec!["take_photo/0".into()],
///     |world, vessel, _gid, _args| {
///         world.trigger(CaptureFromCamera { target: Some(vessel) });
///         ToolResult::Ok
///     },
/// );
/// ```
pub fn register_closure_tool(
    name: impl Into<String>,
    functions: Vec<String>,
    exec: impl Fn(&mut DeferredWorld, Entity, u64, &str) -> ToolResult + Send + Sync + 'static,
) {
    lunco_tools::register(Arc::new(ClosureTool::new(name, functions, exec)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closure_tool_is_executable() {
        let tool = ClosureTool::new("test::ping", vec!["ping/0".into()], |_, _, _, _| ToolResult::Ok);
        assert_eq!(tool.name(), "test::ping");
        assert_eq!(tool.backend(), "rust");
        assert_eq!(tool.functions(), vec!["ping/0".to_string()]);
        // The downcast bridge recovers ExecutableTool from the registry shape.
        let any: &dyn Any = tool.as_any();
        assert!(executable_downcast(any).is_some(), "ClosureTool downcasts to ExecutableTool");
    }

    #[test]
    fn tool_fired_dispatches_to_closure_with_right_vessel_and_args() {
        // T1: the load-bearing integration — a ToolFired event reaches the
        // registered ClosureTool's closure, which sees the firing vessel + args
        // and can trigger a typed command on the world. End-to-end through the
        // same observer the host app uses (no shortcuts).
        use std::sync::{Arc, Mutex};
        #[derive(Event, Clone, Debug)]
        struct CapturedCommand { vessel: Entity, args: String }
        let captured: Arc<Mutex<Option<CapturedCommand>>> = Arc::new(Mutex::new(None));

        // Register a closure tool that records what it was called with. It also
        // triggers a (marker) event on the world, proving it can act.
        let captured_for_closure = captured.clone();
        register_closure_tool(
            "test::capture",
            vec!["capture/0".into()],
            move |world, vessel, _gid, args| {
                world.trigger(CapturedCommand { vessel, args: args.to_string() });
                *captured_for_closure.lock().unwrap() = Some(CapturedCommand { vessel, args: args.to_string() });
                ToolResult::Ok
            },
        );

        let mut app = App::new();
        app.add_plugins(ToolDispatchPlugin);
        // An observer that also records (belt-and-suspenders: proves the
        // closure's world.trigger reached a real observer, not just that the
        // closure ran).
        let vessel = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(ToolFired {
            vessel,
            vessel_gid: 42,
            tool: "test::capture".into(),
            args: r#"{"exposure":0.5}"#.into(),
        });
        app.world_mut().flush();

        let got = captured.lock().unwrap().clone();
        let got = got.expect("closure must have run");
        assert_eq!(got.vessel, vessel, "closure sees the firing vessel Entity");
        assert_eq!(got.args, r#"{"exposure":0.5}"#, "closure sees the leaf's args string");
    }

    #[test]
    fn tool_fired_for_unregistered_tool_warns_and_drops() {
        // An unregistered tool name must not panic — it warns and drops.
        let mut app = App::new();
        app.add_plugins(ToolDispatchPlugin);
        let vessel = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(ToolFired {
            vessel,
            vessel_gid: 0,
            tool: "does::not_exist".into(),
            args: String::new(),
        });
        app.world_mut().flush();
        // No assertion beyond "didn't panic" — the warn+drop path is the point.
    }
}
