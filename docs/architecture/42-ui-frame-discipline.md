# 42 — UI Frame Discipline

> Status: Active · Audience: anyone writing Bevy/egui systems

**TL;DR:** Per-frame work is the anti-default; prefer observers / change-detection / fingerprints / generation-gates.
Push heavy work off-thread or behind a cache; profile before optimizing.

> **Frame *count* is fixed at vsync by design — the lever is per-frame *cost*, not
> redraw frequency.** While focused, both binaries run
> `WinitSettings { focused_mode: UpdateMode::Continuous }`
> (`lunco-luncosim/src/ui/mod.rs:55`, `lunco-modelica/src/bin/lunica.rs:177`), so
> the app redraws *every* vsync interval and never idles while focused — this is
> deliberate (vsync = Fifo present / `requestAnimationFrame` acts as the frame
> timer; see the comment at `ui/mod.rs:41-49`). Reactive/low-power kicks in only
> when **unfocused and not networked**. Consequences a would-be optimizer must
> internalize:
> - **"Idle FPS spikes" is a misnomer while focused** — there is no idle; every
>   frame renders. A spike is one frame doing too much *work*, not the app failing
>   to sleep. Chase per-frame cost (this whole doc + the caching substrates), not
>   redraw scheduling.
> - **`egui::request_repaint()` is near-moot for focused frame count** — Continuous
>   already forces a redraw regardless of who calls it. The ~15 call sites are
>   trigger/animation-gated and matter only in the *unfocused* reactive window.
> - **Switching `focused_mode` to `Reactive` is out of scope** — it was considered
>   and left Continuous on purpose. If you revisit it, it's a frame-*pacing*
>   decision (input latency, vsync interaction, web `requestAnimationFrame`), a
>   different axis from the per-frame-work discipline below. Don't conflate them.

The app ships with a real-time 3D scene, a Modelica simulator, and a
heavyweight egui UI on top. The frame budget is shared — UI work that
looks "cheap in isolation" still competes with the physics step and
the renderer every tick. Three rules:

For Twin-facing retained HTML/CSS-like surfaces, see
[`runtime-authored-ui.md`](runtime-authored-ui.md) for the HUI/Flair contract,
reload loop, and exposure revision boundary. The same frame-budget rules apply:
the authored surface avoids unnecessary rebuilds, but it is still part of the
rendered UI and must be measured with the rest of the frame.

## 1. Per-frame work is the anti-default
Bevy makes it easy to write `Update` systems that do work every tick.
**Do not treat that as the right shape for UI state.** A system that
runs every frame for information that changes once a minute is a bug,
even if the per-frame cost looks small in a profiler — it burns cache
lines, pushes allocations through the frame, and makes it impossible
to reason about where a spike came from. Ask instead:

1. **Does this state change only on an event?** → react to the event.
   Use observers (`app.add_observer(...)`) or `EventReader`.
2. **Does this state change when a resource mutates?** → gate on
   `Res::is_changed()` / `Query::is_changed()` / `Changed<T>` filters.
3. **Is "nothing changed" the common case but detection requires
   comparing a few values?** → stash a fingerprint
   (`Local<Cursor { last_gen, last_hash, ... }>`), early-return when
   it matches, only do the real work on mismatch. This is what
   `refresh_diagnostics` does — cursor holds `(doc_id, ast_gen,
   error_hash)` and skips all allocations on unchanged frames.
4. **Does the work depend on a monotonic counter (document generation,
   sample index, tick number)?** → store the last-seen counter on the
   consumer and re-run only when the producer has advanced it. Phase α
   diagram projection uses this pattern (`last_seen_gen`).

Only use unconditional-every-frame systems for genuinely continuous
work: the renderer, physics stepping, tool animation ticks, smooth
camera easing. Everything else is reactive.

## 2. The UI must stay responsive
The user types, drags, and right-clicks into the same event queue
the physics solver empties. Never block that queue:

- **Keep `Update` systems short.** If a system routinely takes
  >1 ms, break it up, gate it on change, or push the work to a
  background thread / task pool.
- **No synchronous I/O on the UI thread.** Load files, parse large
  sources, and scan directories via `bevy::tasks::AsyncComputeTaskPool`
  and poll the handle with `future::poll_once` each frame. The
  Package Browser's folder scan is the reference implementation.
- **No per-frame allocations in the common path.** `String` clones
  and `Vec` rebuilds that happen on a no-op path are the most
  common offenders — pre-allocate, reuse, or skip entirely.
- **Frame-rate-independent animation.** Anything using time must
  take `dt` from `ui.ctx().input(|i| i.unstable_dt)` (egui) or
  `Time::delta` (bevy). Never assume 60 Hz.

## 3. Heavy work goes off-thread or behind a cache
Parsing a large `.mo` file, rasterising an SVG, indexing an MSL
package — none of these belong on the UI thread every frame. Patterns:

- **One-shot + cache**: global `OnceLock<Mutex<HashMap>>` keyed by
  a stable identifier (path, hash, id). Cache-hit returns a
  `Arc<T>` clone. Reference: `svg_bytes_for` in the canvas panel,
  `msl_component_library` in `visual_diagram.rs`.
- **Background task + poll**: `AsyncComputeTaskPool::get().spawn(...)`
  returns a `Task<T>`; `future::poll_once(&mut task)` in an Update
  system yields the result when ready without blocking. Reference:
  the Package Browser's `handle_package_loading_tasks`.
- **Generation-gated recompute**: the canvas diagram only
  reprojects when the document generation moves; the panel advances
  its `last_seen_gen` to skip echo rebuilds of its own ops.

The same ownership rule applies to the measured presentation paths:

- **USD telemetry projection** keeps its generated-wrapper port map and
  domain-member index in a projection-owned resource. The projector is
  scheduled only while an unprojected prim or a changed generated wrapper
  exists, and runs after Wiring has published that wrapper's
  `DeclaredOutputPorts`. Steady frames do not rebuild maps or clone authored
  path keys, and compile-time wrapper publication cannot be mistaken for a
  missing telemetry port.
- **Graphs** retain the history-to-plot point buffer in the visualization
  owner, keyed by the history fingerprint. A plot host may clone points at the
  `egui_plot` owned-data boundary, but it must not recopy the SignalRegistry
  ring buffer merely because the panel painted again.
- **Canvas edges** retain projected screen geometry by scene generation and
  viewport key. Scene edits, viewport movement, and panel resizing invalidate
  that geometry; selection and tool state remain live draw inputs and do not
  force a route rebuild.
- **Dock anchors** publish all authored slot unions from one dock-tree walk.
  Adding another anchor group must extend that pass rather than add another
  full layout traversal.

## 4. How to decide
Quick checklist before you write a `Update` system:

- [ ] Can I write this as an observer on a specific event? → do that.
- [ ] Can I gate on `Res::is_changed()` or a fingerprint? → do that.
- [ ] Is the work inherently continuous (animation, input, render)? →
      per-frame is fine, but keep it allocation-free.
- [ ] None of the above → it's probably the wrong abstraction.
      Reshape it.

## 5. Profiling subsystem — measure, don't guess

When FPS drops, **do not optimise from code reading.** A frame loop runs the
3D scene + Avian + an embedded egui IDE together, and the dominant cost is
rarely the obvious one. Use the profiling subsystem:

```sh
scripts/perf/profile.sh --release            # build → samply → symbolicated hot functions
scripts/perf/profile.sh --release --diag-only # frame time + GPU adapter only (no sudo)
```

Workflow: **profile → A/B-disable to confirm → fix → re-measure**, in that order.
See `scripts/perf/profile.sh --help` for the full toolkit (setup, reading results, gotchas).

Three regressions/assumptions keep recurring; prefer the by-design fix:

- **Never `(*arc).clone()` a heavy, shared, read-only container** to read it —
  that's a deep copy. Borrow the shared value; clone only the `Arc` when
  ownership is needed. (This was a real ~⅔-of-frame regression in the USD
  cosim path.)
- **Once-per-entity setup belongs in an observer** (`OnAdd<T>`), not a polling
  `run_if(Without<Marker>)` system — the latter re-scans the whole scene every
  frame if any code path forgets to insert the marker. If you must poll, mark
  **every** examined entity, including on `else { continue }` exits.
- **Do not blame diagnostics plugins for physics solver spikes.** Spikes during physics steps are not caused by logging or profiling plugins like `PhysicsTotalDiagnosticsPlugin`, whose overhead is microscopic (measured in microseconds). The cost is driven by the authoritative physics solver configuration (`lunco_physics::DEFAULT_SUBSTEP_COUNT` × solver iterations). Gating or removing the diagnostics plugin merely hides the measurement without resolving the actual cost.

A `run_if`-gated system that still appears in a steady-state profile means its
gate isn't closing — that's the bug, not the cost.

## 6. Four gates that failed, and the shapes that replaced them

Each of these cost 8–12 ms a frame in steady state with nothing changing. They
are recorded because every one of them *looked* gated. The code sites carry the
detail; the shape is the lesson.

**A cadence you cannot forget to state.** A registered producer must declare
when it runs — the gate is a required argument, not something the author
remembers to add. A producer that genuinely must run every frame passes
`every_frame`, which puts the claim at the call site next to its reason, where
review can see it. (`lunco-luncosim-edit/src/ui/mod.rs`)

**Never gate on a hash of the thing you were deciding whether to build.**
`produce_usd_canvas` spent 11 ms/frame building a graph and hashing it only to
learn the graph was unchanged. Gate on a **revision counter** stamped by the
writers: O(1), and it cannot drift from the truth. Keep hashes for assertions.
(`lunco-usd-bevy/src/lib.rs`)

**A `Without<Marker>` filter is not an identity check.** Bevy 0.19 stores
resources as entities, so `Without<GlobalEntityId>` alone minted network
identities for 688 resource entities (`AppTypeRegistry`, every `Assets<T>`).
A missing `Provenance` must fail honestly — no id, and whatever needed one says
so — rather than being auto-filled. (`lunco-core/src/lib.rs`)

**Watch for gates that feed themselves.** An auto-allocated id made an
`Added<GlobalEntityId>` gate fire on the next frame, which despawned and
respawned every edge, which minted fresh ids — a full wiring rebuild every
frame (8.6 ms) with nothing changing. If a system's gate is satisfied by the
system's own output, it is not a gate. (`lunco-usd-sim/src/cosim.rs`)

**Solve on a cadence when the answer changes slowly.** Ephemeris, solar poses,
trajectory alignment, sun light and solar-frame anchoring cost ~10 ms/frame
solved every frame, for increments too small to see.
(`lunco-celestial/src/cadence.rs`)
