# Undo/Redo Libraries — Research & Decision

> **Decision (2026-04):** Keep our own `lunco_doc::DocumentHost`. Do not
> adopt `undo`, `yrs`, or `automerge` as a replacement. Revisit when we
> add op merging/debouncing, or when live-sync scope forces the question.

This note captures the research behind that decision so future contributors
don't re-litigate it every six months.

## The question

When we built `lunco-doc`'s `Document` / `DocumentOp` / `DocumentHost`
(~100 LOC of core code), we needed to answer: is there a mature Rust
library that already does this, so we're not reinventing a wheel?

## Libraries surveyed

### Local undo/redo (non-collab)

| Library | API shape | Notes |
|---|---|---|
| [`undo`](https://docs.rs/undo/) v0.52 | `Edit` trait with `edit(&mut target)` + `undo(&mut target)` methods; `Record` (linear) + `History` (tree); merge support; serde optional | ~56 stars. Maintained since 2016. Boutique-mature. |
| [`redo`](https://crates.io/crates/redo) | Same shape, static dispatch | Same author as `undo`; earlier form. |
| [`undo_2`](https://crates.io/crates/undo_2) | Variant history semantics — rewind-bakes-onto-end rather than truncate | Niche. |

### CRDT-based (collab + undo as byproduct)

| Library | API shape | Notes |
|---|---|---|
| [`yrs`](https://docs.rs/yrs) | Yjs port. `YMap` / `YArray` / `YText` primitives. Undo manager separate project ([NLnet-funded, deadline 2026-06-01](https://nlnet.nl/project/Yrs-Undo/)). | Active, production-adjacent. The path to Omniverse Nucleus-compatible live sync if we want it. |
| [`automerge`](https://docs.rs/automerge/) | `AutoCommit` / `Transaction` API. External [`automerge-repo-undo-redo`](https://github.com/onsetsoftware/automerge-repo-undo-redo) wrapper adds undo/redo. | Mature CRDT from Ink & Switch. Research paper [*Extending Automerge: Undo, Redo, and Move*](https://2023.splashcon.org/details/plf-2023-papers/2/Extending-Automerge-Undo-Redo-and-Move) (SPLASH 2023). |

## API comparison — `undo::Edit` vs our `Document`

```rust
// lunco_doc::Document
fn apply(&mut self, op: Op) -> Result<Op, Error>;   // returns inverse op

// undo::Edit
fn edit(&mut self, target: &mut Target) -> Output;
fn undo(&mut self, target: &mut Target) -> Output;
```

Both shapes express the same concept, but three differences matter for
LunCoSim:

### 1. Ops as pure data vs. stateful commands

Our ops are plain enum values. They can be cloned, printed, compared,
serialized, sent over a network, appended to a replay log. The inverse
is another enum value.

`undo::Edit` is a trait on the *command itself* — an `Edit` is a stateful
object with `edit` and `undo` methods. Commands can close over captured
state. That's ergonomic for local editing but hostile to serialization
(how do you serialize a method pointer?). For Nucleus-style live sync
and replay, you'd end up wrapping `Edit` to extract pure op values back
out.

This is a **first-class concern**. Our own principles doc (Article IV:
*Preserve user intent*) calls out replay/record of user intent as a core
property. Ops-as-data makes that property cheap; ops-as-code makes it
expensive.

### 2. Inverse computed by domain logic in one pass

When `ModelicaOp::RemoveComponent("R1")` runs, the domain code knows
exactly what was removed and returns `AddComponent { ... }` as the
inverse in the same function. One place to reason about the semantics.

`undo::Edit` requires two methods (`edit` and `undo`) that each need
to reason about the same state transition. The usual implementation
pattern stores the "undo data" as fields on the command object when
`edit` runs, then uses it in `undo`. That's two stages of logic for
one conceptual change — opportunities for drift.

### 3. `Result`-typed apply

We reject invalid ops at the apply boundary and return a `DocumentError`.
`undo::Edit` assumes success; failures panic or need ad-hoc encoding in
the `Output` type.

Validation at the op boundary is a **load-bearing property** of our
design: the document never enters a half-mutated state, undo stacks
stay coherent after failed edits. That's stronger than what `undo`
gives out of the box.

## Maturity / adoption signals

| Crate | Stars | Age | Signals |
|---|---|---|---|
| `undo` | ~56 | 9 years | Stable but niche. Not a de-facto standard. |
| `yrs` | Active | 3+ years | Growing, NLnet-funded. Real production use via Yjs compatibility. |
| `automerge` | Large | 5+ years | Research-driven, mature, Ink & Switch. |

`undo` is not a "community standard" whose adoption we'd benefit from
socially. Switching would be lateral, not cumulative.

## When each replaces what we built

**`undo` / `redo` / `undo_2`** — they don't. They're local-only, they
wrap the same ~100 LOC we wrote. The only real gain would be tree-based
`History` and the `merge` pattern, both of which we can port as needed
without pulling in a dependency.

**`yrs` / `automerge`** — these do not *replace* `Document`; they are
candidates for use *inside* specific Document types. For example,
`ModelicaDocument`'s equation-body text could be backed by a `YText`
internally, gaining CRDT semantics for that sub-field. The outer
Document abstraction (typed domain ops, generation counter, headless
testability) stays the same.

Structured typed ops like `AddConnection { from: (c1, p1), to: (c2, p2) }`
have no direct representation in yrs/automerge — you'd encode them as
JSON-like mutations and layer typing on top. That's the right call for
a collaborative text editor; it's not the right call for a Modelica
model editor where the op vocabulary *is* the semantics.

## What we should steal eventually

- **`merge` pattern from `undo`.** `DocumentOp::merge(self, other) -> Option<Self>`
  lets sequential character inserts collapse into one undo step when
  CodeEditor gains granular text ops. Clean API; port, don't import.
- **Branching history from `undo::History`.** Tree-based history is a
  real UX feature (redo past a forked edit). On the roadmap; not
  needed for v1.

## Triggers to revisit this decision

Reopen this question if any of these fire:

1. We actually need op merging/debouncing in CodeEditor → first consider
   adopting `undo::Edit` merge semantics rather than copying.
2. Live-sync scope lands (Omniverse Nucleus integration or browser-based
   collab) → pick `yrs`-inside-Documents vs `automerge`-inside-Documents
   based on which external ecosystem we need protocol compatibility
   with (Yjs for browser editors, Automerge for local-first apps).
3. Our `Document` trait ends up with three+ helper traits for branching,
   merging, saved-point tracking — at that point the size gap with
   `undo` shrinks enough that adoption is a win on LOC alone.

## What we are NOT claiming

- **That we write better code than `undo`.** It's a fine library.
  We're claiming the architectural fit for our specific future (typed
  domain ops, replay, eventual CRDT sub-fields) is different enough
  that adopting would *add* complexity, not reduce it.
- **That `undo`/`yrs`/`automerge` are wrong for anyone else.** In a
  Rust text editor without collab ambitions, `undo` is probably the
  right default.

## See also

- [`10-document-system.md`](../10-document-system.md) — the design this decision serves.
- [`../principles.md`](../principles.md) Article IV — replay/record of
  user intent as a core property.
