//! rhai task trees on the [`lunco_behavior`] kernel.
//!
//! The prelude's task constructors (`seq`/`par_all`/`par_race`/`repeat`/
//! `forever`/`once`/`wait`/…) build PURE DATA maps — policy, inspectable,
//! snapshot-safe. This module is the mechanism that used to be ~100 lines of
//! rhai `__tick*` recursion: [`compile_node`] turns the map tree into a
//! [`lunco_behavior`] tree once per assignment, and the world-bridge ticks it
//! natively every frame. One tick engine (the unit-tested kernel) now serves
//! both the autopilot (`BehaviorSpec`) and scripted tasks; the rhai side keeps
//! only the constructors.
//!
//! Leaves call back into script closures via [`TaskCtx`] — dyn-erased so the
//! tree type is `'static` and can live in [`CompiledTask`] beside the script
//! state. Leaf semantics mirror the retired rhai engine exactly:
//! - `act` runs every tick while the leaf is live (`once` completes on the
//!   first tick, so its action runs once);
//! - explicit leaf kinds select exactly one contract: `once`, `step`, `act_for`,
//!   `act_until_event`, `wait`, `wait_until`, `wait_for`, `wait_for_from`, or
//!   `check`; a missing
//!   kind or a field belonging to another kind is rejected at compile time;
//! - dwell is entry-stamped and cleared on [`Node::reset`] so `repeat`/`forever`
//!   re-dwell; event matching uses name + optional source, with a string source
//!   path re-resolved via `find()` each tick until it matches;
//! - a closure error is surfaced as a diagnostic and the leaf stays `Running`
//!   (the retired engine aborted the whole tick and retried next frame).
//!
//! Composites come straight from the kernel, which also unlocks nodes the rhai
//! engine never had: `sel` (Selector fallback), `retry`, `invert`,
//! `force_ok`/`force_fail`, `reactive_seq`/`reactive_sel`, and the `check`
//! leaf (predicate → Success/Failure) that makes Selector/Retry meaningful
//! from scripts.

use lunco_behavior::{
    BoxNode, Force, Invert, Node, Parallel, ParallelPolicy, ReactiveSelector, ReactiveSequence,
    Repeat, Retry, Selector, Sequence, Status,
};
use rhai::{Dynamic, FnPtr, ImmutableString, Map};

const NODE_KIND_FIELD: &str = "kind";
const ACTION_FIELD: &str = "act";
const DONE_FIELD: &str = "done";
const CHECK_FIELD: &str = "check";
const SECONDS_FIELD: &str = "secs";
const EVENT_FIELD: &str = "event";
const SOURCE_FIELD: &str = "src";
const ITEMS_FIELD: &str = "items";
const BODY_FIELD: &str = "body";
const COUNT_FIELD: &str = "n";
const INTERNAL_TASK_MARKER: &str = "__bt";
const KIND_ONCE: &str = "once";
const KIND_STEP: &str = "step";
const KIND_ACT_FOR: &str = "act_for";
const KIND_ACT_UNTIL_EVENT: &str = "act_until_event";
const KIND_WAIT: &str = "wait";
const KIND_WAIT_UNTIL: &str = "wait_until";
const KIND_WAIT_FOR: &str = "wait_for";
const KIND_WAIT_FOR_FROM: &str = "wait_for_from";
const KIND_CHECK: &str = "check";
const KIND_SEQ: &str = "seq";
const KIND_SEL: &str = "sel";
const KIND_ALL: &str = "all";
const KIND_RACE: &str = "race";
const KIND_REPEAT: &str = "repeat";
const KIND_FOREVER: &str = "forever";
const KIND_RETRY: &str = "retry";
const KIND_INVERT: &str = "invert";
const KIND_FORCE_OK: &str = "force_ok";
const KIND_FORCE_FAIL: &str = "force_fail";
const KIND_REACTIVE_SEQ: &str = "reactive_seq";
const KIND_REACTIVE_SEL: &str = "reactive_sel";

/// The closed task-node schema after the dynamic Rhai boundary has been
/// crossed. Every node, including leaves, must carry one of these explicit
/// discriminators. Raw strings are accepted only by `parse`; execution matches
/// this enum, so missing or unknown kinds cannot select a heuristic fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskKind {
    Once,
    Step,
    ActFor,
    ActUntilEvent,
    Wait,
    WaitUntil,
    WaitFor,
    WaitForFrom,
    Check,
    Sequence,
    Selector,
    All,
    Race,
    Repeat,
    Forever,
    Retry,
    Invert,
    ForceOk,
    ForceFail,
    ReactiveSequence,
    ReactiveSelector,
}

impl TaskKind {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            KIND_ONCE => Ok(Self::Once),
            KIND_STEP => Ok(Self::Step),
            KIND_ACT_FOR => Ok(Self::ActFor),
            KIND_ACT_UNTIL_EVENT => Ok(Self::ActUntilEvent),
            KIND_WAIT => Ok(Self::Wait),
            KIND_WAIT_UNTIL => Ok(Self::WaitUntil),
            KIND_WAIT_FOR => Ok(Self::WaitFor),
            KIND_WAIT_FOR_FROM => Ok(Self::WaitForFrom),
            KIND_CHECK => Ok(Self::Check),
            KIND_SEQ => Ok(Self::Sequence),
            KIND_SEL => Ok(Self::Selector),
            KIND_ALL => Ok(Self::All),
            KIND_RACE => Ok(Self::Race),
            KIND_REPEAT => Ok(Self::Repeat),
            KIND_FOREVER => Ok(Self::Forever),
            KIND_RETRY => Ok(Self::Retry),
            KIND_INVERT => Ok(Self::Invert),
            KIND_FORCE_OK => Ok(Self::ForceOk),
            KIND_FORCE_FAIL => Ok(Self::ForceFail),
            KIND_REACTIVE_SEQ => Ok(Self::ReactiveSequence),
            KIND_REACTIVE_SEL => Ok(Self::ReactiveSelector),
            other => Err(format!("unknown task node kind `{other}`")),
        }
    }
}

/// World access a ticking task tree needs. Dyn-erased (`BoxNode<dyn TaskCtx>`)
/// so trees are `'static`; the concrete impl borrows the engine + AST for the
/// duration of one tick.
pub trait TaskCtx {
    /// Sim-time seconds (the `elapsed_seconds()` the retired engine used).
    fn now(&self) -> f64;
    /// Events buffered since the last tick, as `(name, source-gid)` (`0` =
    /// global emitter).
    fn events(&self) -> &[(ImmutableString, i64)];
    /// Resolve an entity path/name to a gid (`find()`; `-1` = not found).
    fn resolve(&mut self, path: &str) -> i64;
    /// Call an action closure with the host gid. Errors are recorded by the
    /// impl (surfaced as a script diagnostic after the tick).
    fn call_action(&mut self, f: &FnPtr) -> Result<(), TaskCallbackError>;
    /// Call a predicate closure with the host gid; must return a bool.
    fn call_pred(&mut self, f: &FnPtr) -> Result<bool, TaskCallbackError>;
}

#[derive(Debug, Clone, Copy)]
pub struct TaskCallbackError;

/// A compiled task tree plus its completion latch. The kernel's `Sequence`
/// resets itself on terminal (so it re-runs under `Repeat`), which means the
/// ROOT would restart every tick after finishing — `done` latches the first
/// terminal status, mirroring the retired engine's `__task_done` + single
/// `TASK_COMPLETE` emit.
pub struct CompiledTask {
    /// Identity marker also stamped into the source map as `__bt`, so a script
    /// re-assigning `this.task` (fresh map, no marker) triggers a recompile.
    pub id: i64,
    pub tree: BoxNode<dyn TaskCtx>,
    pub done: bool,
}

impl CompiledTask {
    pub fn new(id: i64, tree: BoxNode<dyn TaskCtx>) -> Self {
        Self {
            id,
            tree,
            done: false,
        }
    }

    /// Placeholder for a spec that failed to compile: latched `done` so the
    /// compile error reports once, not every tick.
    pub fn poisoned(id: i64) -> Self {
        Self {
            id,
            tree: Box::new(Sequence::new(Vec::new())),
            done: true,
        }
    }
}

/// The event-name/source match a `wait_for` / `wait_for_from` leaf performs.
enum SrcSpec {
    /// `wait_for(name)` — any emitter.
    Any,
    /// `wait_for_from(name, gid)` — exact emitter.
    Gid(i64),
    /// `wait_for_from(name, "path")` — emitter resolved lazily every tick
    /// (the entity may not exist when the tree is built at `on_start`).
    Path(String),
}

/// Leaf node: the explicit `once`/`step`/`act_for`/`act_until_event`/`wait`/`wait_until`/
/// `wait_for`/`check` map shapes.
struct Leaf {
    act: Option<FnPtr>,
    done: Option<FnPtr>,
    check: Option<FnPtr>,
    secs: Option<f64>,
    event: Option<ImmutableString>,
    src: SrcSpec,
    /// Dwell entry time; lazily stamped, cleared on reset so a repeated body
    /// dwells afresh each iteration.
    t0: Option<f64>,
}

impl Node<dyn TaskCtx> for Leaf {
    // `Ctx = dyn TaskCtx` carries an implicit `'static` bound (trees outlive
    // any one tick), so the signature must spell it out — and ctx impls must
    // OWN their resources (`Arc`s), not borrow the runtime.
    fn tick(&mut self, ctx: &mut (dyn TaskCtx + 'static)) -> Status {
        if let Some(act) = &self.act {
            if ctx.call_action(act).is_err() {
                return Status::Running; // error surfaced by ctx; retry next tick
            }
        }
        if let Some(check) = &self.check {
            return match ctx.call_pred(check) {
                Ok(true) => Status::Success,
                Ok(false) => Status::Failure,
                Err(TaskCallbackError) => Status::Running,
            };
        }
        if let Some(done) = &self.done {
            return match ctx.call_pred(done) {
                Ok(true) => Status::Success,
                Ok(false) => Status::Running,
                Err(TaskCallbackError) => Status::Running,
            };
        }
        if let Some(secs) = self.secs {
            let t0 = *self.t0.get_or_insert_with(|| ctx.now());
            return if ctx.now() - t0 >= secs {
                Status::Success
            } else {
                Status::Running
            };
        }
        if let Some(name) = &self.event {
            let want = match &self.src {
                SrcSpec::Any => None,
                SrcSpec::Gid(g) => Some(*g),
                SrcSpec::Path(p) => {
                    let p = p.clone();
                    Some(ctx.resolve(&p))
                }
            };
            let hit = ctx
                .events()
                .iter()
                .any(|(n, s)| n == name && want.is_none_or(|w| *s == w));
            return if hit {
                Status::Success
            } else {
                Status::Running
            };
        }
        Status::Success // bare / `once` — complete on the first tick
    }

    fn reset(&mut self) {
        self.t0 = None;
    }
}

/// Extract an anonymous closure field, erroring on a present-but-wrong type or
/// a named `Fn("...")` pointer. Task callbacks need the method-bound `this`
/// state, so accepting a named pointer would create a second callback contract
/// and fail later with a misleading arity error.
fn fnptr_field(m: &Map, key: &str) -> Result<Option<FnPtr>, String> {
    match m.get(key) {
        None => Ok(None),
        Some(v) if v.is_unit() => Ok(None),
        Some(v) => {
            let pointer = v.clone().try_cast::<FnPtr>().ok_or_else(|| {
                format!("task leaf `{key}` must be an anonymous closure `|me| ...`")
            })?;
            if !pointer.is_anonymous() {
                return Err(format!(
                    "task leaf `{key}` must be an anonymous closure `|me| ...`; named `Fn(\"...\")` callbacks are not task leaves"
                ));
            }
            Ok(Some(pointer))
        }
    }
}

fn required_fnptr_field(m: &Map, key: &str) -> Result<FnPtr, String> {
    fnptr_field(m, key)?.ok_or_else(|| format!("task node missing `{key}`"))
}

fn seconds_field(m: &Map) -> Result<f64, String> {
    let value = m
        .get(SECONDS_FIELD)
        .ok_or_else(|| format!("task node missing `{SECONDS_FIELD}`"))?;
    let seconds = value
        .as_float()
        .or_else(|_| value.as_int().map(|i| i as f64))
        .map_err(|t| format!("task node `{SECONDS_FIELD}` must be a number, got `{t}`"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!(
            "task node `{SECONDS_FIELD}` must be a finite non-negative number"
        ));
    }
    Ok(seconds)
}

fn event_field(m: &Map) -> Result<ImmutableString, String> {
    let value = m
        .get(EVENT_FIELD)
        .ok_or_else(|| format!("task node missing `{EVENT_FIELD}`"))?;
    value
        .clone()
        .into_immutable_string()
        .map_err(|t| format!("task node `{EVENT_FIELD}` must be a string, got `{t}`"))
}

fn source_field(m: &Map) -> Result<SrcSpec, String> {
    let value = m
        .get(SOURCE_FIELD)
        .ok_or_else(|| format!("task node missing `{SOURCE_FIELD}`"))?;
    if value.is_string() {
        Ok(SrcSpec::Path(value.to_string()))
    } else {
        Ok(SrcSpec::Gid(value.as_int().map_err(|t| {
            format!("task node `{SOURCE_FIELD}` must be a gid or path, got `{t}`")
        })?))
    }
}

fn validate_fields(m: &Map, allowed: &[&str]) -> Result<(), String> {
    for key in m.keys() {
        let key = key.as_str();
        if key != NODE_KIND_FIELD && key != INTERNAL_TASK_MARKER && !allowed.contains(&key) {
            return Err(format!(
                "task field `{key}` is not valid for this node kind"
            ));
        }
    }
    Ok(())
}

fn leaf_for(kind: TaskKind, m: &Map) -> Result<Leaf, String> {
    let (act, done, check, secs, event, src) = match kind {
        TaskKind::Once => (
            Some(required_fnptr_field(m, ACTION_FIELD)?),
            None,
            None,
            None,
            None,
            SrcSpec::Any,
        ),
        TaskKind::Step => (
            Some(required_fnptr_field(m, ACTION_FIELD)?),
            Some(required_fnptr_field(m, DONE_FIELD)?),
            None,
            None,
            None,
            SrcSpec::Any,
        ),
        TaskKind::ActFor => (
            Some(required_fnptr_field(m, ACTION_FIELD)?),
            None,
            None,
            Some(seconds_field(m)?),
            None,
            SrcSpec::Any,
        ),
        TaskKind::ActUntilEvent => (
            Some(required_fnptr_field(m, ACTION_FIELD)?),
            None,
            None,
            None,
            Some(event_field(m)?),
            source_field(m)?,
        ),
        TaskKind::Wait => (
            None,
            None,
            None,
            Some(seconds_field(m)?),
            None,
            SrcSpec::Any,
        ),
        TaskKind::WaitUntil => (
            None,
            Some(required_fnptr_field(m, DONE_FIELD)?),
            None,
            None,
            None,
            SrcSpec::Any,
        ),
        TaskKind::WaitFor => (None, None, None, None, Some(event_field(m)?), SrcSpec::Any),
        TaskKind::WaitForFrom => (
            None,
            None,
            None,
            None,
            Some(event_field(m)?),
            source_field(m)?,
        ),
        TaskKind::Check => (
            None,
            None,
            Some(required_fnptr_field(m, CHECK_FIELD)?),
            None,
            None,
            SrcSpec::Any,
        ),
        _ => return Err("not a task leaf kind".to_string()),
    };
    Ok(Leaf {
        act,
        done,
        check,
        secs,
        event,
        src,
        t0: None,
    })
}

/// Compile one explicitly tagged Rhai task node into a kernel node. Missing or
/// unknown `kind` values are errors; there is no leaf/composite inference.
pub fn compile_node(v: &Dynamic) -> Result<BoxNode<dyn TaskCtx>, String> {
    let m = v
        .read_lock::<Map>()
        .ok_or_else(|| format!("task node must be a map, got `{}`", v.type_name()))?;

    let raw_kind = m
        .get(NODE_KIND_FIELD)
        .and_then(|k| k.clone().into_immutable_string().ok());
    let raw_kind = raw_kind
        .ok_or_else(|| format!("task node requires string `{NODE_KIND_FIELD}` discriminator"))?;
    let kind = TaskKind::parse(&raw_kind)?;

    let allowed = match kind {
        TaskKind::Once => &[ACTION_FIELD][..],
        TaskKind::Step => &[ACTION_FIELD, DONE_FIELD][..],
        TaskKind::ActFor => &[ACTION_FIELD, SECONDS_FIELD][..],
        TaskKind::ActUntilEvent => &[ACTION_FIELD, EVENT_FIELD, SOURCE_FIELD][..],
        TaskKind::Wait => &[SECONDS_FIELD][..],
        TaskKind::WaitUntil => &[DONE_FIELD][..],
        TaskKind::WaitFor => &[EVENT_FIELD][..],
        TaskKind::WaitForFrom => &[EVENT_FIELD, SOURCE_FIELD][..],
        TaskKind::Check => &[CHECK_FIELD][..],
        TaskKind::Sequence
        | TaskKind::Selector
        | TaskKind::All
        | TaskKind::Race
        | TaskKind::ReactiveSequence
        | TaskKind::ReactiveSelector => &[ITEMS_FIELD][..],
        TaskKind::Repeat | TaskKind::Retry => &[COUNT_FIELD, BODY_FIELD][..],
        TaskKind::Forever | TaskKind::Invert | TaskKind::ForceOk | TaskKind::ForceFail => {
            &[BODY_FIELD][..]
        }
    };
    validate_fields(&m, allowed)?;

    if matches!(
        kind,
        TaskKind::Once
            | TaskKind::Step
            | TaskKind::ActFor
            | TaskKind::ActUntilEvent
            | TaskKind::Wait
            | TaskKind::WaitUntil
            | TaskKind::WaitFor
            | TaskKind::WaitForFrom
            | TaskKind::Check
    ) {
        return Ok(Box::new(leaf_for(kind, &m)?));
    }

    let children = |field: &str| -> Result<Vec<BoxNode<dyn TaskCtx>>, String> {
        let items = m
            .get(field)
            .ok_or_else(|| format!("task node missing `{field}`"))?;
        let arr = items
            .read_lock::<rhai::Array>()
            .ok_or_else(|| format!("task `{field}` must be an array"))?;
        arr.iter().map(compile_node).collect()
    };
    let body = || -> Result<BoxNode<dyn TaskCtx>, String> {
        compile_node(
            m.get(BODY_FIELD)
                .ok_or_else(|| "task node missing `body`".to_string())?,
        )
    };
    let count = || -> Result<usize, String> {
        let n = m
            .get(COUNT_FIELD)
            .and_then(|n| n.as_int().ok())
            .ok_or_else(|| "task node requires integer `n`".to_string())?;
        usize::try_from(n).map_err(|_| "task node `n` must be non-negative".to_string())
    };

    Ok(match kind {
        TaskKind::Once
        | TaskKind::Step
        | TaskKind::ActFor
        | TaskKind::ActUntilEvent
        | TaskKind::Wait
        | TaskKind::WaitUntil
        | TaskKind::WaitFor
        | TaskKind::WaitForFrom
        | TaskKind::Check => unreachable!("leaf kinds are returned before composite parsing"),
        TaskKind::Sequence => Box::new(Sequence::new(children(ITEMS_FIELD)?)),
        TaskKind::Selector => Box::new(Selector::new(children(ITEMS_FIELD)?)),
        TaskKind::All => Box::new(Parallel::new(
            ParallelPolicy::RequireAll,
            children(ITEMS_FIELD)?,
        )),
        TaskKind::Race => Box::new(Parallel::new(
            ParallelPolicy::RequireOne,
            children(ITEMS_FIELD)?,
        )),
        TaskKind::Repeat => Box::new(Repeat::times(count()?, body()?)),
        TaskKind::Forever => Box::new(Repeat::forever(body()?)),
        TaskKind::Retry => Box::new(Retry::times(count()?, body()?)),
        TaskKind::Invert => Box::new(Invert::new(body()?)),
        TaskKind::ForceOk => Box::new(Force::succeed(body()?)),
        TaskKind::ForceFail => Box::new(Force::fail(body()?)),
        TaskKind::ReactiveSequence => Box::new(ReactiveSequence::new(children(ITEMS_FIELD)?)),
        TaskKind::ReactiveSelector => Box::new(ReactiveSelector::new(children(ITEMS_FIELD)?)),
    })
}

#[cfg(test)]
mod tests {
    //! Semantics parity with the retired rhai `__tick*` engine, via a fake ctx
    //! (no Engine): closures are keyed by curried tag since a bare test can't
    //! build callable FnPtrs — leaves under test use explicit `kind` shapes,
    //! which cover the sequencing/dwell/event logic the engine owned. Closure
    //! invocation itself is covered by the world-bridge integration path.
    use super::*;

    struct FakeCtx {
        now: f64,
        events: Vec<(ImmutableString, i64)>,
    }
    impl TaskCtx for FakeCtx {
        fn now(&self) -> f64 {
            self.now
        }
        fn events(&self) -> &[(ImmutableString, i64)] {
            &self.events
        }
        fn resolve(&mut self, _path: &str) -> i64 {
            42
        }
        fn call_action(&mut self, _f: &FnPtr) -> Result<(), TaskCallbackError> {
            Ok(())
        }
        fn call_pred(&mut self, _f: &FnPtr) -> Result<bool, TaskCallbackError> {
            Ok(true)
        }
    }

    fn map(pairs: &[(&str, Dynamic)]) -> Dynamic {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert((*k).into(), v.clone());
        }
        Dynamic::from_map(m)
    }

    fn tagged(kind: &str, pairs: &[(&str, Dynamic)]) -> Dynamic {
        let mut m = Map::new();
        m.insert(NODE_KIND_FIELD.into(), kind.into());
        for (k, v) in pairs {
            m.insert((*k).into(), v.clone());
        }
        Dynamic::from_map(m)
    }

    fn anonymous_fn() -> FnPtr {
        let engine = rhai::Engine::new();
        let ast = engine.compile("fn make() { |me| me }").unwrap();
        engine
            .call_fn(&mut rhai::Scope::new(), &ast, "make", ())
            .unwrap()
    }

    #[test]
    fn seq_of_dwells_advances_with_time() {
        // seq([ wait(1.0), wait(2.0) ]) — done only after 3 s of cumulative dwell.
        let tree = tagged(
            "seq",
            &[(
                "items",
                Dynamic::from_array(vec![
                    tagged("wait", &[("secs", Dynamic::from_float(1.0))]),
                    tagged("wait", &[("secs", Dynamic::from_float(2.0))]),
                ]),
            )],
        );
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx {
            now: 0.0,
            events: vec![],
        };
        assert_eq!(node.tick(&mut ctx), Status::Running); // stamps t0 of leg 1
        ctx.now = 1.5;
        assert_eq!(node.tick(&mut ctx), Status::Running); // leg 1 done, leg 2 stamps 1.5
        ctx.now = 3.0;
        assert_eq!(node.tick(&mut ctx), Status::Running); // 1.5 s into a 2 s dwell
        ctx.now = 3.6;
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn wait_for_matches_name_and_source() {
        // wait_for_from("GO", "path") — src resolves to 42 in FakeCtx.
        let tree = tagged(
            "wait_for_from",
            &[("event", "GO".into()), ("src", "launcher".into())],
        );
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx {
            now: 0.0,
            events: vec![("GO".into(), 7)],
        };
        assert_eq!(
            node.tick(&mut ctx),
            Status::Running,
            "wrong source must not match"
        );
        ctx.events = vec![("HALT".into(), 42)];
        assert_eq!(
            node.tick(&mut ctx),
            Status::Running,
            "wrong name must not match"
        );
        ctx.events = vec![("GO".into(), 42)];
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn act_until_event_requires_the_named_source() {
        let tree = tagged(
            "act_until_event",
            &[
                ("act", Dynamic::from(anonymous_fn())),
                ("event", "GO".into()),
                ("src", "launcher".into()),
            ],
        );
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx {
            now: 0.0,
            events: vec![("GO".into(), 7)],
        };
        assert_eq!(node.tick(&mut ctx), Status::Running);
        ctx.events = vec![("GO".into(), 42)];
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn repeat_re_dwells_each_iteration() {
        // repeat(2, wait(1.0)) — the dwell's t0 must clear between iterations.
        let tree = tagged(
            "repeat",
            &[
                ("n", Dynamic::from_int(2)),
                (
                    "body",
                    tagged("wait", &[("secs", Dynamic::from_float(1.0))]),
                ),
            ],
        );
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx {
            now: 0.0,
            events: vec![],
        };
        assert_eq!(node.tick(&mut ctx), Status::Running);
        ctx.now = 1.0;
        assert_eq!(node.tick(&mut ctx), Status::Running); // iter 1 done, iter 2 restamps
        ctx.now = 1.5;
        assert_eq!(
            node.tick(&mut ctx),
            Status::Running,
            "second dwell must restart"
        );
        ctx.now = 2.4;
        assert_eq!(
            node.tick(&mut ctx),
            Status::Running,
            "restamped at 1.5 → done at 2.5"
        );
        ctx.now = 2.6;
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn race_finishes_on_first_done() {
        let tree = tagged(
            "race",
            &[(
                "items",
                Dynamic::from_array(vec![
                    tagged("wait", &[("secs", Dynamic::from_float(10.0))]),
                    tagged("wait_for", &[("event", "GO".into())]),
                ]),
            )],
        );
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx {
            now: 0.0,
            events: vec![],
        };
        assert_eq!(node.tick(&mut ctx), Status::Running);
        ctx.events = vec![("GO".into(), 0)];
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn bad_shapes_error_at_compile() {
        assert!(compile_node(&Dynamic::from_int(3)).is_err());
        assert!(compile_node(&map(&[("items", Dynamic::from_array(vec![]))])).is_err()); // missing kind
        assert!(compile_node(&tagged("warp", &[])).is_err()); // unknown kind
        assert!(compile_node(&tagged("once", &[("act", Dynamic::from_int(5))])).is_err()); // act not a closure
        assert!(compile_node(&tagged(
            "once",
            &[("act", Dynamic::from(FnPtr::new("named_action").unwrap()))],
        ))
        .is_err()); // named functions are not task callbacks
        assert!(compile_node(&tagged(
            "act_for",
            &[
                ("act", Dynamic::from_int(5)),
                ("secs", Dynamic::from_float(1.0))
            ],
        ))
        .is_err()); // act_for still requires a closure
        assert!(compile_node(&tagged("wait", &[("secs", Dynamic::from_float(-1.0))])).is_err());
        assert!(compile_node(&tagged(
            "once",
            &[
                ("act", Dynamic::from_int(5)),
                ("secs", Dynamic::from_float(1.0))
            ],
        ))
        .is_err()); // fields from another kind are rejected
        assert!(compile_node(&tagged(
            "repeat",
            &[
                ("n", Dynamic::from_int(-1)),
                (
                    "body",
                    tagged("wait", &[("secs", Dynamic::from_float(0.0))])
                )
            ],
        ))
        .is_err()); // counts are explicit, never clamped as a fallback
    }
}
