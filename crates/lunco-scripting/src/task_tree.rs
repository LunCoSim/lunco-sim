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
//! - done precedence: `done` predicate → `secs` dwell (entry-stamped, cleared
//!   on [`Node::reset`] so `repeat`/`forever` re-dwell) → `event` match
//!   (name + optional source; a string source is a path re-resolved via
//!   `find()` each tick until it matches, entities may not exist at build
//!   time) → bare leaf = complete immediately;
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
    fn call_action(&mut self, f: &FnPtr) -> Result<(), ()>;
    /// Call a predicate closure with the host gid; must return a bool.
    fn call_pred(&mut self, f: &FnPtr) -> Result<bool, ()>;
}

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
        Self { id, tree, done: false }
    }

    /// Placeholder for a spec that failed to compile: latched `done` so the
    /// compile error reports once, not every tick.
    pub fn poisoned(id: i64) -> Self {
        Self { id, tree: Box::new(Sequence::new(Vec::new())), done: true }
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

/// Leaf node: the `once`/`step`/`wait`/`wait_until`/`wait_for`/`check` map
/// shapes, with the same field precedence as the retired `__tick_leaf`.
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
                Err(()) => Status::Running,
            };
        }
        if let Some(done) = &self.done {
            return match ctx.call_pred(done) {
                Ok(true) => Status::Success,
                Ok(false) => Status::Running,
                Err(()) => Status::Running,
            };
        }
        if let Some(secs) = self.secs {
            let t0 = *self.t0.get_or_insert_with(|| ctx.now());
            return if ctx.now() - t0 >= secs { Status::Success } else { Status::Running };
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
            return if hit { Status::Success } else { Status::Running };
        }
        Status::Success // bare / `once` — complete on the first tick
    }

    fn reset(&mut self) {
        self.t0 = None;
    }
}

/// Extract an optional `FnPtr` field, erroring on a present-but-wrong type
/// (the silent-skip alternative turns a typo'd `#{ act: 5 }` into a no-op).
fn fnptr_field(m: &Map, key: &str) -> Result<Option<FnPtr>, String> {
    match m.get(key) {
        None => Ok(None),
        Some(v) if v.is_unit() => Ok(None),
        Some(v) => v
            .clone()
            .try_cast::<FnPtr>()
            .map(Some)
            .ok_or_else(|| format!("task leaf `{key}` must be a closure/function pointer")),
    }
}

/// Compile one node of the rhai map tree into a kernel node. Maps with a `k`
/// field are composites; anything else is a leaf.
pub fn compile_node(v: &Dynamic) -> Result<BoxNode<dyn TaskCtx>, String> {
    let m = v
        .read_lock::<Map>()
        .ok_or_else(|| format!("task node must be a map, got `{}`", v.type_name()))?;

    let kind = m.get("k").and_then(|k| k.clone().into_immutable_string().ok());
    let Some(kind) = kind else {
        // Leaf — mirror the retired `__tick_leaf` field vocabulary.
        let secs = match m.get("secs") {
            None => None,
            Some(v) if v.is_unit() => None,
            Some(v) => Some(v.as_float().or_else(|_| v.as_int().map(|i| i as f64)).map_err(
                |t| format!("task leaf `secs` must be a number, got `{t}`"),
            )?),
        };
        let event = match m.get("event") {
            None => None,
            Some(v) if v.is_unit() => None,
            Some(v) => Some(
                v.clone()
                    .into_immutable_string()
                    .map_err(|t| format!("task leaf `event` must be a string, got `{t}`"))?,
            ),
        };
        let src = match m.get("src") {
            None => SrcSpec::Any,
            Some(v) if v.is_unit() => SrcSpec::Any,
            Some(v) if v.is_string() => SrcSpec::Path(v.to_string()),
            Some(v) => SrcSpec::Gid(
                v.as_int()
                    .map_err(|t| format!("task leaf `src` must be a gid or path, got `{t}`"))?,
            ),
        };
        return Ok(Box::new(Leaf {
            act: fnptr_field(&m, "act")?,
            done: fnptr_field(&m, "done")?,
            check: fnptr_field(&m, "check")?,
            secs,
            event,
            src,
            t0: None,
        }));
    };

    let children = |field: &str| -> Result<Vec<BoxNode<dyn TaskCtx>>, String> {
        let items = m
            .get(field)
            .ok_or_else(|| format!("task `{kind}` node missing `{field}`"))?;
        let arr = items
            .read_lock::<rhai::Array>()
            .ok_or_else(|| format!("task `{kind}` `{field}` must be an array"))?;
        arr.iter().map(compile_node).collect()
    };
    let body = || -> Result<BoxNode<dyn TaskCtx>, String> {
        compile_node(m.get("body").ok_or_else(|| format!("task `{kind}` node missing `body`"))?)
    };
    let count = || -> Result<usize, String> {
        m.get("n")
            .and_then(|n| n.as_int().ok())
            .map(|n| n.max(0) as usize)
            .ok_or_else(|| format!("task `{kind}` node missing integer `n`"))
    };

    Ok(match kind.as_str() {
        "seq" => Box::new(Sequence::new(children("items")?)),
        "sel" => Box::new(Selector::new(children("items")?)),
        "all" => Box::new(Parallel::new(ParallelPolicy::RequireAll, children("items")?)),
        "race" => Box::new(Parallel::new(ParallelPolicy::RequireOne, children("items")?)),
        "repeat" => Box::new(Repeat::times(count()?, body()?)),
        "forever" => Box::new(Repeat::forever(body()?)),
        "retry" => Box::new(Retry::times(count()?, body()?)),
        "invert" => Box::new(Invert::new(body()?)),
        "force_ok" => Box::new(Force::succeed(body()?)),
        "force_fail" => Box::new(Force::fail(body()?)),
        "reactive_seq" => Box::new(ReactiveSequence::new(children("items")?)),
        "reactive_sel" => Box::new(ReactiveSelector::new(children("items")?)),
        other => return Err(format!("unknown task node kind `{other}`")),
    })
}

#[cfg(test)]
mod tests {
    //! Semantics parity with the retired rhai `__tick*` engine, via a fake ctx
    //! (no Engine): closures are keyed by curried tag since a bare test can't
    //! build callable FnPtrs — leaves under test use `secs`/`event`/bare shapes,
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
        fn call_action(&mut self, _f: &FnPtr) -> Result<(), ()> {
            Ok(())
        }
        fn call_pred(&mut self, _f: &FnPtr) -> Result<bool, ()> {
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

    #[test]
    fn seq_of_dwells_advances_with_time() {
        // seq([ wait(1.0), wait(2.0) ]) — done only after 3 s of cumulative dwell.
        let tree = map(&[
            ("k", "seq".into()),
            (
                "items",
                Dynamic::from_array(vec![
                    map(&[("secs", Dynamic::from_float(1.0))]),
                    map(&[("secs", Dynamic::from_float(2.0))]),
                ]),
            ),
        ]);
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx { now: 0.0, events: vec![] };
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
        let tree = map(&[("event", "GO".into()), ("src", "launcher".into())]);
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx { now: 0.0, events: vec![("GO".into(), 7)] };
        assert_eq!(node.tick(&mut ctx), Status::Running, "wrong source must not match");
        ctx.events = vec![("HALT".into(), 42)];
        assert_eq!(node.tick(&mut ctx), Status::Running, "wrong name must not match");
        ctx.events = vec![("GO".into(), 42)];
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn repeat_re_dwells_each_iteration() {
        // repeat(2, wait(1.0)) — the dwell's t0 must clear between iterations.
        let tree = map(&[
            ("k", "repeat".into()),
            ("n", Dynamic::from_int(2)),
            ("body", map(&[("secs", Dynamic::from_float(1.0))])),
        ]);
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx { now: 0.0, events: vec![] };
        assert_eq!(node.tick(&mut ctx), Status::Running);
        ctx.now = 1.0;
        assert_eq!(node.tick(&mut ctx), Status::Running); // iter 1 done, iter 2 restamps
        ctx.now = 1.5;
        assert_eq!(node.tick(&mut ctx), Status::Running, "second dwell must restart");
        ctx.now = 2.4;
        assert_eq!(node.tick(&mut ctx), Status::Running, "restamped at 1.5 → done at 2.5");
        ctx.now = 2.6;
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn race_finishes_on_first_done() {
        let tree = map(&[
            ("k", "race".into()),
            (
                "items",
                Dynamic::from_array(vec![
                    map(&[("secs", Dynamic::from_float(10.0))]),
                    map(&[("event", "GO".into())]),
                ]),
            ),
        ]);
        let mut node = compile_node(&tree).unwrap();
        let mut ctx = FakeCtx { now: 0.0, events: vec![] };
        assert_eq!(node.tick(&mut ctx), Status::Running);
        ctx.events = vec![("GO".into(), 0)];
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn bad_shapes_error_at_compile() {
        assert!(compile_node(&Dynamic::from_int(3)).is_err());
        assert!(compile_node(&map(&[("k", "seq".into())])).is_err()); // no items
        assert!(compile_node(&map(&[("k", "warp".into())])).is_err()); // unknown kind
        assert!(compile_node(&map(&[("act", Dynamic::from_int(5))])).is_err()); // act not a closure
    }
}
