//! Language- and engine-agnostic **behaviour-tree** kernel for LunCoSim.
//!
//! This crate holds the per-tick *mechanism* of autonomy — the tree node model,
//! its tick traversal, and the pure event-matching predicates — with **no
//! dependency on any scripting runtime (rhai/python), avian, or bevy**. That's
//! the whole point: the mechanism lives here once, deterministic and unit-tested,
//! and each language binding in `lunco-scripting` drives the *same* engine by
//! supplying a context (`Ctx`) that gives world access and invokes its callables.
//! Mission *policy* (which waypoints, which steps) stays in the thin per-language
//! prelude; only the reusable machinery moves here.
//!
//! ## What's here now
//! - [`Status`], the [`Node`] trait, and the [`Action`] closure leaf.
//! - Composites: [`Sequence`], [`Selector`], [`Parallel`] (`RequireAll`/
//!   `RequireOne` = the current `par_all`/`par_race`), [`Repeat`] (`times`/
//!   `forever`), and [`Retry`] (`times`/`forever` — the failure-side mirror of
//!   `Repeat`).
//! - **Reactive** composites: [`ReactiveSequence`] / [`ReactiveSelector`], which
//!   re-evaluate their children from the first one every tick (guards stay live)
//!   rather than latching the running child — the "do B **while** A holds" and
//!   priority-arbiter nodes robotics behaviours need.
//! - **Decorators**: [`Invert`] (negate a condition) and [`Force`] (`succeed`/
//!   `fail` — swallow a failure or force an abort).
//! - Pure [`events`] predicates (`event_matches`, `entered_zone`, …).
//!
//! The kernel is deliberately clock-free, so *time-bounded* decorators (timeout,
//! cooldown) and pose/sensor leaves live in the consuming layer that owns a
//! context — e.g. `lunco-autopilot`'s `Timeout` reads its `DriveCtx.now`.
//!
//! ## Growing further (later)
//! A utility/priority selector that *scores* children, and named reusable
//! subtrees, are each just new [`Node`] impls — the tick contract above does not
//! change. Every kernel node maps 1:1 onto the rhai/JSON `BehaviorSpec` in
//! `lunco-autopilot`, which authors the tree as data and compiles the leaves.

pub mod events;
pub mod node;

pub use events::{entered_zone, event_matches, exited_zone, zone_of};
pub use node::{
    Action, BoxNode, Force, Invert, Node, Parallel, ParallelPolicy, ReactiveSelector,
    ReactiveSequence, Repeat, Retry, Selector, Sequence, Status,
};
