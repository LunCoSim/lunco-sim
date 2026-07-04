# Ports system: resolve→handle — Substrate D

*Part of the efficiency/maintainability architecture. See
`caching-and-precompute-strategy.md`. The port substrate itself is documented in
`lunco_core::ports`.*

## The cost

Every co-sim endpoint is a `(Entity, port name)` addressed through the
[`PortRegistry`], which folds over registered [`PortBackend`]s (first match
wins). A name read/write therefore pays, per call:

- one `world.get::<T>` **presence check per backend** until the owner is found, and
- for the avian backend, up to **six group-presence `get`s + a name scan**
  (`find_avian_port` walked `AVIAN` groups, each gated on a component).

The propagation master runs this **every tick** for every wire source and every
target. For a rover — position/velocity read and `force_y` written each tick on
avian bodies behind the `SimComponent` backend — that's the dominant port cost.
The strings were already removed (0.3's `CompiledWiring`); the remaining cost is
the per-tick backend fold + group scan.

## The model: FMI valueReference (resolve once, exchange by handle)

FMI never exchanges by variable *name* on the hot path — it resolves names to
integer **value references** once, then reads/writes by reference. Substrate D
brings that to ports:

- A backend may expose an **optional** fast path on `PortBackend`:
  `resolve_output`/`resolve_input` (name → opaque `u64` **slot**) and
  `read_slot`/`write_slot` (exchange by slot). `None` ⇒ no fast path.
- The registry resolves an endpoint to a `ResolvedPort { backend, slot }` once,
  then `read_resolved`/`write_resolved` dispatch straight to the owning backend —
  **no fold, no group scan**.
- `ResolvedPort` is **process-local** (like an FMI value reference / a port slot):
  the `slot` is backend-private and MUST NOT be serialized or sent on the wire —
  resolve fresh on every peer. This keeps it inside the determinism firewall.

### Who opts in

Only backends behind a multi-group scan benefit, so only they implement the fast
path:

| Backend | Fast path? | Why |
|---|---|---|
| `SimComponent` (Modelica map) | no (`None`) | registered first — a name read is already one `get` + map lookup |
| **avian** (bodies/joints/sensors) | **yes** | slot = `(group_index << 16) \| port_index` into `AVIAN`; collapses the 6-group scan to one component access |
| `PhysicalPort` / `DigitalPort` | no | single fixed port on one component |
| FSW command (map) | no (for now) | map-backed; a fast path needs a name interner (slot can't carry the string) — a documented follow-up |

The name-based avian ops are now **derived** from resolve→slot (the old
`find_avian_port` duplication is gone): `read_output = resolve_output ∘
read_slot`. This is the "name-based API derived from the handle model, no
per-backend duplication" endgame, applied where it pays.

## Correctness

- **Precedence-preserving.** `resolve_*` walks backends in registration order and
  stops at the FIRST owner, so a lower-precedence fast-path backend (avian) can
  never shadow a higher-precedence name-only owner (`SimComponent`) when a name
  collides on one entity. If the winner has no fast path, resolution returns
  `None` and the caller uses the name path — same backend, same result.
  - Outputs are readable, so `read_output.is_some()` detects ownership.
  - Inputs may be **write-only** (avian `force_y` reads `None`), so resolution
    also accepts a backend's own `resolve_input` as the write-ownership
    authority. Precedence holds for our registration order (the readable-input
    backends `SimComponent`/FSW precede the write-only avian one).
- **Stale-handle safe.** A cached handle whose component was removed/swapped since
  the last rebuild makes `read_resolved`/`write_resolved` return `None`/`false`;
  the propagate loop then falls back to the name path (short-circuiting so a
  successful slot write never double-writes). Behaviour is identical to the
  pre-resolve master.
- **Invalidation.** Handles are cached in `CompiledWiring`, rebuilt (via
  `RebuildOnChange`) when the `SimConnection` set changes. A component swap
  without a wiring change is covered by the per-tick fallback above.

## Where it's wired

`lunco-cosim/systems/propagate.rs` — `CompiledWiring::rebuild` resolves every wire
source and every distinct target once; the accumulate/write phases exchange by
handle. One-shot name callers (API `Get/SetPort`, scripting, the inspector) are
unchanged — they don't need resolution and pay no migration cost.

## Follow-ups

- **Map-backed fast path.** A small process-local name interner would let
  `SimComponent`/FSW resolve to a slot too (removing the fold for FSW drive-command
  writes, which currently fall back through the avian scan). Add if profiling
  shows it matters.
- **Register-order invariant.** `ResolvedPort.backend` is an index into the
  registry's fixed startup registration; no backend may be registered after the
  first resolve. (All registration is in plugin `build` today.)
