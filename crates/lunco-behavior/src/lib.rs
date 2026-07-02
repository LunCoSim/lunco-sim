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
//! ## What's here now (simple)
//! - [`Status`], the [`Node`] trait, and composites [`Sequence`], [`Selector`],
//!   [`Parallel`] (`RequireAll`/`RequireOne` = the current `par_all`/`par_race`),
//!   [`Repeat`] (`times`/`forever`), and the [`Action`] closure leaf.
//! - Pure [`events`] predicates (`event_matches`, `entered_zone`, …).
//!
//! ## Growing to hierarchical behaviour trees (later)
//! Decorators (invert/succeed/cooldown/timeout), a priority/utility selector, and
//! named reusable subtrees are each just new [`Node`] impls — the tick contract
//! above does not change. The existing rhai task tree
//! (`seq`/`par_all`/`par_race`/`repeat`/`forever`) maps 1:1 onto the composites
//! here; wiring the rhai/python leaves as [`Action`]s (via a `Callable` context)
//! is the next integration step, not part of this kernel.

pub mod events;
pub mod node;

pub use events::{entered_zone, event_matches, exited_zone, zone_of};
pub use node::{Action, Node, Parallel, ParallelPolicy, Repeat, Selector, Sequence, Status};
