//! The behaviour-tree node model: a [`Status`], the object-safe [`Node`] trait,
//! and the standard nodes —
//! - composites [`Sequence`], [`Selector`], [`Parallel`], [`Repeat`], [`Retry`];
//! - reactive composites [`ReactiveSequence`], [`ReactiveSelector`], which re-tick
//!   from the first child every tick so guards stay live;
//! - decorators [`Invert`] and [`Force`];
//! - the [`Action`] closure leaf.
//!
//! Every node is generic over a host-supplied context `Ctx` — the tree carries
//! no world/engine/language dependency of its own. The scripting layer supplies
//! a `Ctx` giving world access + a way to invoke a rhai/python callable, and
//! wraps each script leaf in an [`Action`]. That keeps the engine here
//! deterministic, unit-testable, and shared by every runtime.
//!
//! Composites reset themselves on a terminal result (`Success`/`Failure`) so a
//! finished subtree is fresh when re-entered (e.g. under a [`Repeat`]). The
//! reactive composites additionally reset the children they skipped this tick, so
//! a preempted branch never carries stale state into its next run.
//!
//! Hierarchical growth (later): a priority/utility selector that *scores* children
//! and named reusable subtrees are just new `Node` impls — no change to this core.
//! Time-bounded decorators (timeout/cooldown) need a clock, so they live in the
//! consuming layer that owns a context (see `lunco-autopilot`).

/// Result of ticking a [`Node`] once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Still working; tick again next time.
    Running,
    /// Completed successfully.
    Success,
    /// Completed with failure.
    Failure,
}

/// A behaviour-tree node. Object-safe so composites can hold
/// `Box<dyn Node<Ctx>>` children of mixed concrete types.
pub trait Node<Ctx: ?Sized> {
    /// Advance the node by one tick against the host context.
    fn tick(&mut self, ctx: &mut Ctx) -> Status;
    /// Return the node to its initial state (called when a parent restarts it).
    fn reset(&mut self) {}
}

/// A boxed child node. `Send + Sync` so a whole tree can live in an ECS
/// `Component` (Bevy requires it) and be ticked from a system — the kernel stays
/// engine-free, but its containers are usable per-entity. Leaves that capture only
/// `Send + Sync` data (the common case) satisfy this automatically.
pub type BoxNode<Ctx> = Box<dyn Node<Ctx> + Send + Sync>;

/// A leaf that runs a closure each tick and returns its [`Status`]. The scripting
/// layer wraps a rhai/python callable in one of these.
pub struct Action<F> {
    run: F,
}

impl<F> Action<F> {
    /// Wrap a `FnMut(&mut Ctx) -> Status` closure as a leaf node.
    pub fn new(run: F) -> Self {
        Self { run }
    }
}

impl<Ctx: ?Sized, F: FnMut(&mut Ctx) -> Status> Node<Ctx> for Action<F> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        (self.run)(ctx)
    }
}

/// Runs children in order; fails on the first child failure, succeeds when all
/// children have succeeded. The classic "do A then B then C" node.
pub struct Sequence<Ctx: ?Sized> {
    children: Vec<BoxNode<Ctx>>,
    current: usize,
}

impl<Ctx: ?Sized> Sequence<Ctx> {
    /// Build a sequence over the given ordered children.
    pub fn new(children: Vec<BoxNode<Ctx>>) -> Self {
        Self { children, current: 0 }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Sequence<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        while self.current < self.children.len() {
            match self.children[self.current].tick(ctx) {
                Status::Running => return Status::Running,
                Status::Success => self.current += 1,
                Status::Failure => {
                    self.reset();
                    return Status::Failure;
                }
            }
        }
        self.reset();
        Status::Success
    }

    fn reset(&mut self) {
        self.current = 0;
        for c in &mut self.children {
            c.reset();
        }
    }
}

/// Runs children in order; succeeds on the first child success, fails only when
/// every child has failed. The "fallback" node — try A, else B, else C.
pub struct Selector<Ctx: ?Sized> {
    children: Vec<BoxNode<Ctx>>,
    current: usize,
}

impl<Ctx: ?Sized> Selector<Ctx> {
    /// Build a selector (fallback) over the given ordered children.
    pub fn new(children: Vec<BoxNode<Ctx>>) -> Self {
        Self { children, current: 0 }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Selector<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        while self.current < self.children.len() {
            match self.children[self.current].tick(ctx) {
                Status::Running => return Status::Running,
                Status::Failure => self.current += 1,
                Status::Success => {
                    self.reset();
                    return Status::Success;
                }
            }
        }
        self.reset();
        Status::Failure
    }

    fn reset(&mut self) {
        self.current = 0;
        for c in &mut self.children {
            c.reset();
        }
    }
}

/// Completion rule for [`Parallel`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParallelPolicy {
    /// Succeed when ALL children succeed; fail as soon as any child fails
    /// (`par_all`).
    RequireAll,
    /// Succeed as soon as ANY child succeeds; fail when all children fail
    /// (`par_race`).
    RequireOne,
}

/// Ticks all still-running children each tick and resolves by [`ParallelPolicy`].
/// Each child's terminal result is latched so it isn't re-ticked after finishing.
pub struct Parallel<Ctx: ?Sized> {
    children: Vec<BoxNode<Ctx>>,
    latched: Vec<Status>,
    policy: ParallelPolicy,
}

impl<Ctx: ?Sized> Parallel<Ctx> {
    /// Build a parallel node with the given completion policy.
    pub fn new(policy: ParallelPolicy, children: Vec<BoxNode<Ctx>>) -> Self {
        let latched = vec![Status::Running; children.len()];
        Self { children, latched, policy }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Parallel<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        for i in 0..self.children.len() {
            if self.latched[i] == Status::Running {
                self.latched[i] = self.children[i].tick(ctx);
            }
        }
        let any = |s: Status| self.latched.contains(&s);
        let all = |s: Status| self.latched.iter().all(|l| *l == s);
        let outcome = match self.policy {
            ParallelPolicy::RequireAll => {
                if any(Status::Failure) {
                    Some(Status::Failure)
                } else if all(Status::Success) {
                    Some(Status::Success)
                } else {
                    None
                }
            }
            ParallelPolicy::RequireOne => {
                if any(Status::Success) {
                    Some(Status::Success)
                } else if all(Status::Failure) {
                    Some(Status::Failure)
                } else {
                    None
                }
            }
        };
        match outcome {
            Some(s) => {
                self.reset();
                s
            }
            None => Status::Running,
        }
    }

    fn reset(&mut self) {
        for l in &mut self.latched {
            *l = Status::Running;
        }
        for c in &mut self.children {
            c.reset();
        }
    }
}

/// Re-runs a child to `Success` a fixed number of times, or forever. Any child
/// `Failure` fails the repeat immediately.
pub struct Repeat<Ctx: ?Sized> {
    child: BoxNode<Ctx>,
    /// `None` = forever; `Some(n)` = until the child has succeeded `n` times.
    target: Option<usize>,
    done: usize,
}

impl<Ctx: ?Sized> Repeat<Ctx> {
    /// Repeat `child` until it has succeeded `count` times, then succeed.
    pub fn times(count: usize, child: BoxNode<Ctx>) -> Self {
        Self { child, target: Some(count), done: 0 }
    }

    /// Repeat `child` forever (only a child failure ends it).
    pub fn forever(child: BoxNode<Ctx>) -> Self {
        Self { child, target: None, done: 0 }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Repeat<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        // A zero-count repeat is trivially complete.
        if self.target == Some(0) {
            return Status::Success;
        }
        match self.child.tick(ctx) {
            Status::Running => Status::Running,
            Status::Failure => {
                self.reset();
                Status::Failure
            }
            Status::Success => {
                self.done += 1;
                if self.target.is_some_and(|n| self.done >= n) {
                    self.reset();
                    Status::Success
                } else {
                    self.child.reset();
                    Status::Running
                }
            }
        }
    }

    fn reset(&mut self) {
        self.done = 0;
        self.child.reset();
    }
}

/// Retries a child on `Failure` up to a fixed number of attempts, or forever — the
/// mirror of [`Repeat`] (which re-runs on `Success`). A child `Success` ends the
/// retry with `Success`; once the failure budget is spent it ends with `Failure`.
/// Use for a flaky maneuver that's worth another go (re-attempt a dock, a burn, a
/// grasp) before conceding to a fallback branch.
pub struct Retry<Ctx: ?Sized> {
    child: BoxNode<Ctx>,
    /// `None` = retry forever; `Some(n)` = give up with `Failure` after `n` failures.
    budget: Option<usize>,
    failed: usize,
}

impl<Ctx: ?Sized> Retry<Ctx> {
    /// Retry `child` until it succeeds, giving up (`Failure`) after `count` failures.
    pub fn times(count: usize, child: BoxNode<Ctx>) -> Self {
        Self { child, budget: Some(count), failed: 0 }
    }

    /// Retry `child` forever until it succeeds (a failure never ends it).
    pub fn forever(child: BoxNode<Ctx>) -> Self {
        Self { child, budget: None, failed: 0 }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Retry<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        // A zero-attempt retry has no budget to spend → immediate failure.
        if self.budget == Some(0) {
            return Status::Failure;
        }
        match self.child.tick(ctx) {
            Status::Running => Status::Running,
            Status::Success => {
                self.reset();
                Status::Success
            }
            Status::Failure => {
                self.failed += 1;
                if self.budget.is_some_and(|n| self.failed >= n) {
                    self.reset();
                    Status::Failure
                } else {
                    self.child.reset();
                    Status::Running
                }
            }
        }
    }

    fn reset(&mut self) {
        self.failed = 0;
        self.child.reset();
    }
}

// ── Decorators (single-child wrappers) ───────────────────────────────────────

/// Inverts a child's terminal result: `Success` ↔ `Failure` (`Running` passes
/// through). The standard way to use a condition as its negation — wrap an
/// `arrived`-style guard so a branch runs *until* the condition holds.
pub struct Invert<Ctx: ?Sized> {
    child: BoxNode<Ctx>,
}

impl<Ctx: ?Sized> Invert<Ctx> {
    /// Wrap `child`, negating its terminal result.
    pub fn new(child: BoxNode<Ctx>) -> Self {
        Self { child }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Invert<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        match self.child.tick(ctx) {
            Status::Success => Status::Failure,
            Status::Failure => Status::Success,
            Status::Running => Status::Running,
        }
    }

    fn reset(&mut self) {
        self.child.reset();
    }
}

/// Maps a child's terminal result to a fixed one (`Running` passes through).
/// [`Force::succeed`] makes a best-effort child never fail its parent sequence
/// (optional recovery, cosmetic step); [`Force::fail`] forces an abort regardless
/// of the child's own result.
pub struct Force<Ctx: ?Sized> {
    child: BoxNode<Ctx>,
    forced: Status,
}

impl<Ctx: ?Sized> Force<Ctx> {
    /// Wrap `child` so any terminal result becomes `Success`.
    pub fn succeed(child: BoxNode<Ctx>) -> Self {
        Self { child, forced: Status::Success }
    }

    /// Wrap `child` so any terminal result becomes `Failure`.
    pub fn fail(child: BoxNode<Ctx>) -> Self {
        Self { child, forced: Status::Failure }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for Force<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        match self.child.tick(ctx) {
            Status::Running => Status::Running,
            _ => self.forced,
        }
    }

    fn reset(&mut self) {
        self.child.reset();
    }
}

// ── Reactive composites (re-evaluate from the first child every tick) ─────────

/// Like [`Sequence`], but **reactive**: it re-ticks its children from the first one
/// on every tick instead of latching the running child, so earlier *condition*
/// children are re-checked continuously. This is the "do B **while** A holds"
/// node — `reactive_sequence([safe?, drive])` re-evaluates `safe?` each frame and
/// bails the instant it fails, where a plain [`Sequence`] would latch `drive` as
/// running and never look at `safe?` again. Returns `Running` while a child is
/// running, `Failure` on the first child failure, `Success` only when every child
/// succeeds within one tick.
pub struct ReactiveSequence<Ctx: ?Sized> {
    children: Vec<BoxNode<Ctx>>,
}

impl<Ctx: ?Sized> ReactiveSequence<Ctx> {
    /// Build a reactive sequence over the given ordered children.
    pub fn new(children: Vec<BoxNode<Ctx>>) -> Self {
        Self { children }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for ReactiveSequence<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        for i in 0..self.children.len() {
            match self.children[i].tick(ctx) {
                Status::Failure => {
                    self.reset();
                    return Status::Failure;
                }
                Status::Running => {
                    // Later children didn't run this tick; drop any state they held
                    // from a previous frame so re-entry is clean.
                    for c in &mut self.children[i + 1..] {
                        c.reset();
                    }
                    return Status::Running;
                }
                Status::Success => {}
            }
        }
        self.reset();
        Status::Success
    }

    fn reset(&mut self) {
        for c in &mut self.children {
            c.reset();
        }
    }
}

/// Like [`Selector`], but **reactive**: it re-ticks from the highest-priority child
/// every tick, so a higher-priority option can preempt a lower one mid-run. This is
/// the priority-arbiter node — `reactive_selector([avoid_if_blocked, cruise])`
/// re-checks the obstacle guard each frame and switches to avoidance the instant it
/// trips, then back to cruise when clear. Returns `Success` on the first child
/// success, `Running` while a child is running, `Failure` only when every child
/// fails within one tick.
pub struct ReactiveSelector<Ctx: ?Sized> {
    children: Vec<BoxNode<Ctx>>,
}

impl<Ctx: ?Sized> ReactiveSelector<Ctx> {
    /// Build a reactive selector (priority fallback) over the given children.
    pub fn new(children: Vec<BoxNode<Ctx>>) -> Self {
        Self { children }
    }
}

impl<Ctx: ?Sized> Node<Ctx> for ReactiveSelector<Ctx> {
    fn tick(&mut self, ctx: &mut Ctx) -> Status {
        for i in 0..self.children.len() {
            match self.children[i].tick(ctx) {
                Status::Success => {
                    self.reset();
                    return Status::Success;
                }
                Status::Running => {
                    // A higher-priority child took over; reset the lower-priority
                    // children so a preempted running branch starts fresh next time.
                    for c in &mut self.children[i + 1..] {
                        c.reset();
                    }
                    return Status::Running;
                }
                Status::Failure => {}
            }
        }
        self.reset();
        Status::Failure
    }

    fn reset(&mut self) {
        for c in &mut self.children {
            c.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A leaf that returns `Running` for `delay` ticks, then a terminal result.
    struct Countdown {
        delay: u32,
        left: u32,
        end: Status,
    }
    impl Countdown {
        fn new(delay: u32, end: Status) -> BoxNode<()> {
            Box::new(Self { delay, left: delay, end })
        }
    }
    impl Node<()> for Countdown {
        fn tick(&mut self, _: &mut ()) -> Status {
            if self.left == 0 {
                self.end
            } else {
                self.left -= 1;
                Status::Running
            }
        }
        fn reset(&mut self) {
            self.left = self.delay;
        }
    }

    fn run(node: &mut dyn Node<()>, max: u32) -> Status {
        let mut ctx = ();
        for _ in 0..max {
            match node.tick(&mut ctx) {
                Status::Running => continue,
                s => return s,
            }
        }
        Status::Running
    }

    #[test]
    fn sequence_runs_in_order_and_succeeds() {
        let mut s = Sequence::new(vec![
            Countdown::new(1, Status::Success),
            Countdown::new(2, Status::Success),
        ]);
        assert_eq!(run(&mut s, 10), Status::Success);
    }

    #[test]
    fn sequence_fails_on_first_child_failure() {
        let mut s = Sequence::new(vec![
            Countdown::new(0, Status::Failure),
            Countdown::new(0, Status::Success),
        ]);
        assert_eq!(run(&mut s, 10), Status::Failure);
    }

    #[test]
    fn selector_takes_first_success() {
        let mut s = Selector::new(vec![
            Countdown::new(0, Status::Failure),
            Countdown::new(1, Status::Success),
            Countdown::new(0, Status::Failure),
        ]);
        assert_eq!(run(&mut s, 10), Status::Success);
    }

    #[test]
    fn parallel_require_all_waits_for_slowest() {
        let mut p = Parallel::new(
            ParallelPolicy::RequireAll,
            vec![
                Countdown::new(1, Status::Success),
                Countdown::new(3, Status::Success),
            ],
        );
        // Fails before tick 4 would be wrong; must succeed by tick 4.
        assert_eq!(run(&mut p, 10), Status::Success);
    }

    #[test]
    fn parallel_require_one_takes_fastest() {
        let mut p = Parallel::new(
            ParallelPolicy::RequireOne,
            vec![
                Countdown::new(5, Status::Success),
                Countdown::new(1, Status::Success),
            ],
        );
        assert_eq!(run(&mut p, 3), Status::Success);
    }

    #[test]
    fn repeat_times_then_succeeds() {
        let mut r = Repeat::times(3, Countdown::new(0, Status::Success));
        // 3 successes with no Running gaps → succeeds on the 3rd tick.
        let mut ctx = ();
        assert_eq!(r.tick(&mut ctx), Status::Running);
        assert_eq!(r.tick(&mut ctx), Status::Running);
        assert_eq!(r.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn repeat_forever_never_succeeds_but_propagates_failure() {
        let mut r = Repeat::forever(Countdown::new(0, Status::Success));
        let mut ctx = ();
        for _ in 0..100 {
            assert_eq!(r.tick(&mut ctx), Status::Running);
        }
        let mut f = Repeat::forever(Countdown::new(0, Status::Failure));
        assert_eq!(f.tick(&mut ctx), Status::Failure);
    }

    #[test]
    fn action_leaf_runs_closure() {
        let mut n = 0;
        let mut a = Action::new(|_: &mut ()| {
            n += 1;
            if n >= 2 { Status::Success } else { Status::Running }
        });
        let mut ctx = ();
        assert_eq!(a.tick(&mut ctx), Status::Running);
        assert_eq!(a.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn invert_swaps_terminals_passes_running() {
        let mut ctx = ();
        assert_eq!(Invert::new(Countdown::new(0, Status::Success)).tick(&mut ctx), Status::Failure);
        assert_eq!(Invert::new(Countdown::new(0, Status::Failure)).tick(&mut ctx), Status::Success);
        // Running passes straight through.
        assert_eq!(Invert::new(Countdown::new(1, Status::Success)).tick(&mut ctx), Status::Running);
    }

    #[test]
    fn force_maps_any_terminal_to_fixed() {
        let mut ctx = ();
        // A failing child never fails its parent under Force::succeed.
        assert_eq!(Force::succeed(Countdown::new(0, Status::Failure)).tick(&mut ctx), Status::Success);
        // A succeeding child still fails under Force::fail.
        assert_eq!(Force::fail(Countdown::new(0, Status::Success)).tick(&mut ctx), Status::Failure);
        // Running is not a terminal, so it passes through.
        assert_eq!(Force::succeed(Countdown::new(2, Status::Failure)).tick(&mut ctx), Status::Running);
    }

    #[test]
    fn retry_gives_up_after_budget_but_takes_a_success() {
        let mut ctx = ();
        // times(2): two failures exhaust the budget → Failure.
        let mut r = Retry::times(2, Countdown::new(0, Status::Failure));
        assert_eq!(r.tick(&mut ctx), Status::Running); // 1st failure → retry
        assert_eq!(r.tick(&mut ctx), Status::Failure); // 2nd failure → give up

        // A child that fails once then would-succeed: Retry re-runs it. Here a leaf
        // that succeeds on its 2nd tick, under times(3), succeeds (never exhausts).
        let mut n = 0;
        let flaky = Box::new(Action::new(move |_: &mut ()| {
            n += 1;
            if n >= 2 { Status::Success } else { Status::Failure }
        })) as BoxNode<()>;
        let mut r2 = Retry::times(3, flaky);
        assert_eq!(r2.tick(&mut ctx), Status::Running); // fail #1 → retry
        assert_eq!(r2.tick(&mut ctx), Status::Success); // now succeeds
    }

    #[test]
    fn reactive_sequence_rechecks_the_guard_every_tick() {
        // guard: Success for the first 2 ticks, then Failure (a condition that goes
        // false while the action is still running). action: always Running.
        let mut g = 0;
        let guard = Box::new(Action::new(move |_: &mut ()| {
            g += 1;
            if g <= 2 { Status::Success } else { Status::Failure }
        })) as BoxNode<()>;
        let action = Countdown::new(999, Status::Success); // effectively "always Running"
        let mut seq = ReactiveSequence::new(vec![guard, action]);
        let mut ctx = ();
        assert_eq!(seq.tick(&mut ctx), Status::Running); // guard ok, action running
        assert_eq!(seq.tick(&mut ctx), Status::Running); // guard ok, action running
        // A plain Sequence would have latched the running action and NEVER re-check
        // the guard; the reactive one re-evaluates it and bails.
        assert_eq!(seq.tick(&mut ctx), Status::Failure); // guard now false → Failure
    }

    #[test]
    fn reactive_selector_lets_a_higher_priority_child_preempt() {
        // high-priority: Failure for the first 2 ticks, then Success (a condition
        // that trips on). low-priority: always Running (the default action).
        let mut h = 0;
        let high = Box::new(Action::new(move |_: &mut ()| {
            h += 1;
            if h <= 2 { Status::Failure } else { Status::Success }
        })) as BoxNode<()>;
        let low = Countdown::new(999, Status::Success); // always Running
        let mut sel = ReactiveSelector::new(vec![high, low]);
        let mut ctx = ();
        assert_eq!(sel.tick(&mut ctx), Status::Running); // high fails → low runs
        assert_eq!(sel.tick(&mut ctx), Status::Running); // high fails → low runs
        // Reactive: the high-priority child is re-checked and preempts the low one.
        assert_eq!(sel.tick(&mut ctx), Status::Success); // high now succeeds
    }
}
