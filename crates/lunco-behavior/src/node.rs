//! The behaviour-tree node model: a [`Status`], the object-safe [`Node`] trait,
//! and the standard composites ([`Sequence`], [`Selector`], [`Parallel`],
//! [`Repeat`]) plus closure leaf adapters ([`Action`]).
//!
//! Every node is generic over a host-supplied context `Ctx` — the tree carries
//! no world/engine/language dependency of its own. The scripting layer supplies
//! a `Ctx` giving world access + a way to invoke a rhai/python callable, and
//! wraps each script leaf in an [`Action`]. That keeps the engine here
//! deterministic, unit-testable, and shared by every runtime.
//!
//! Composites reset themselves on a terminal result (`Success`/`Failure`) so a
//! finished subtree is fresh when re-entered (e.g. under a [`Repeat`]).
//!
//! Hierarchical growth (later): decorators (invert/succeed/cooldown), a
//! priority/utility selector, and named reusable subtrees are all just new
//! `Node` impls — no change to this core.

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
        let any = |s: Status| self.latched.iter().any(|l| *l == s);
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
}
