# lunco-doc

Document System foundation for LunCoSim — `Document`, `DocumentOp`, and
`DocumentHost` with built-in undo/redo. Zero runtime dependencies;
UI-free; headless-capable.

This crate defines the **shape** of canonical, mutable, observable
structured artifacts used throughout LunCoSim (Modelica models, USD
scenes, SysML blocks, missions, connection graphs). Domain crates
provide concrete implementations; apps compose them inside a Twin.

**Unix convention.** A Document *is* a file. We do not invent a
container format — `.mo` files, `.usda` stages, `.sysml` sources
are the on-disk form of their respective Documents. Opaque binary
files (PNG, GLB, WAV) are *not* Documents; they're file references
tracked by the Twin and edited externally. See
[`10-document-system.md`](../../docs/architecture/10-document-system.md)
§ 2a for the full Document / file reference / Endpoint distinction.

## Core types

| Type | Role |
|------|------|
| [`DocumentId`] | Stable `u64` identifier for a Document |
| [`DocumentOp`] | Marker trait — every Op type implements it |
| [`Document`] | Per-domain trait: `id`, `generation`, `apply(op) -> inverse` |
| [`DocumentHost<D>`] | Wraps a Document + undo/redo stacks |
| [`DocumentError`] | Fallible-apply error type |

## Minimal usage

```rust
use lunco_doc::{Document, DocumentOp, DocumentHost, DocumentError, DocumentId};

struct Counter { id: DocumentId, value: i32, generation: u64 }

#[derive(Clone, Debug)]
enum CounterOp { Inc(i32) }
impl DocumentOp for CounterOp {}

impl Document for Counter {
    type Op = CounterOp;
    fn id(&self) -> DocumentId { self.id }
    fn generation(&self) -> u64 { self.generation }
    fn apply(&mut self, op: CounterOp) -> Result<CounterOp, DocumentError> {
        let CounterOp::Inc(n) = op;
        self.value += n;
        self.generation += 1;
        Ok(CounterOp::Inc(-n))  // inverse op
    }
}

let mut host = DocumentHost::new(Counter {
    id: DocumentId::new(1), value: 0, generation: 0,
});
host.apply(CounterOp::Inc(5)).unwrap();
assert_eq!(host.document().value, 5);
host.undo().unwrap();
assert_eq!(host.document().value, 0);
host.redo().unwrap();
assert_eq!(host.document().value, 5);
```

## Design rules for Document implementations

1. **`apply` must be all-or-nothing.** On failure, the document must be
   unchanged and the error returned. On success, mutation and
   generation-bump are atomic.
2. **Return the exact inverse.** Applying the returned op must reverse
   the change. Undo correctness depends on this invariant.
3. **Bump `generation` on every successful `apply`** — including those
   triggered by undo/redo. Views that key on generation get a fresh
   signal every time state changes.
4. **Keep ops small and composable.** Batched edits can be modeled as
   separate ops that apply sequentially. Large ops are harder to
   invert correctly.
5. **Validate eagerly.** Reject bad ops at `apply` entry; don't partially
   mutate then rollback.

## What this crate does NOT do (yet)

- **Op serialization.** No `Serialize`/`Deserialize` bounds on `DocumentOp`
  yet. Added when persistence or collaboration require it.
- **Bevy integration.** `DocumentHost` is a plain struct, not a
  `Component`. Apps that need ECS integration wrap it themselves.
- **Change notification beyond generation.** A generation bump is the
  only signal. Fine-grained events may be added later (Bevy Events or
  callbacks) if panels need them.
- **Cross-document transactions.** An op applies to one Document.
  Transactions across multiple Documents are handled at a higher level
  (Twin transaction stack in `lunco-twin`, planned).
- **Save / load.** Serialization to disk is the concern of `lunco-twin`
  and each domain crate, not of `lunco-doc`.
- **Built-in `TextDocument` / `BinaryDocument`.** Intentionally absent.
  Domain crates define their own concrete types (`ModelicaDocument`,
  `UsdDocument`, ...). A generic text or binary type would be
  speculative until a real caller needs it.
- **File references and endpoints.** Out of scope — `lunco-doc` is
  about *ops on structured artifacts*, not arbitrary files or remote
  resources. See `10-document-system.md` § 2a.

## Forward compatibility: live sync (Nucleus, Yjs, CRDT)

The `apply` / undo / redo loop today assumes a single authoritative
order — correct for local editing. The trait is kept minimal so that
collaborative extensions (Omniverse Nucleus for USD, Yjs for text,
CRDT-style merge for structured ops) can be added additively when
the time comes:

- Local ops stay the same — `apply` is unchanged.
- Remote ops arrive via a future `apply_remote` / `merge` path.
- Stable ids on structural ops + commutativity hooks enable CRDT
  semantics without rewriting domain code.
- Binary live-sync (streaming texture updates) will live behind a
  *different* trait — we won't stretch `Document` to cover opaque
  blobs with weak op semantics.

We're not building any of this now. We're ensuring the design
doesn't foreclose it.

## Why not `undo` / `yrs` / `automerge`?

We evaluated the obvious Rust candidates ([`undo`](https://docs.rs/undo/)
for local history; [`yrs`](https://docs.rs/yrs) and
[`automerge`](https://docs.rs/automerge/) for CRDT-based collab) and
kept our own `DocumentHost`. Core reason: our ops are **pure enum
values** (serializable, replayable, network-transportable), and
`apply` returns the **inverse op as data** computed in one pass by
domain logic. `undo::Edit` puts methods on the command object (hostile
to serialization); `yrs` / `automerge` operate on JSON-like blobs
(no typed domain ops like `AddConnection { from, to }`).

Full rationale + triggers to revisit:
[`docs/architecture/research/undo-redo-libraries.md`](../../docs/architecture/research/undo-redo-libraries.md).

## Tests

```bash
cargo test -p lunco-doc
```

14 unit tests + 1 doctest cover: apply, undo, redo, generation,
error-on-invalid-op, multi-step round-trip, new-op-clears-redo.

## Crate graph

```
lunco-doc            ← this crate (no runtime deps)
   ▲
   │ used by
   ├── lunco-twin    ← Twin container, DocumentRegistry, manifest
   ├── lunco-ui      ← DocumentView<D> trait + widgets
   └── domain crates (lunco-modelica, lunco-usd, ...) — each defines
                     its own Document + Op types
```
