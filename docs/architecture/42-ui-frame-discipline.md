# 42 — UI Frame Discipline

> Status: Active · Audience: anyone writing Bevy/egui systems

**TL;DR:** Per-frame work is the anti-default; prefer observers / change-detection / fingerprints / generation-gates.
Push heavy work off-thread or behind a cache; profile before optimizing.

> **Frame *count* is fixed at vsync by design — the lever is per-frame *cost*, not
> redraw frequency.** While focused, both binaries run
> `WinitSettings { focused_mode: UpdateMode::Continuous }`
> (`lunco-sandbox/src/ui/mod.rs:55`, `lunco-modelica/src/bin/lunica.rs:177`), so
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

Full reference: [`scripts/perf/README.md`](../../scripts/perf/README.md)
(toolkit, setup, how to read results, mechanics gotchas). Workflow:
**profile → A/B-disable to confirm → fix → re-measure**, in that order.

Two regressions keep recurring; prefer the by-design fix:

- **Never `(*arc).clone()` a heavy, shared, read-only container** (e.g. a USD
  `TextReader`) to read it — that's a deep copy. Borrow `&*arc`; share via
  `Arc`. (This was a real ~⅔-of-frame regression in the USD cosim path.)
- **Once-per-entity setup belongs in an observer** (`OnAdd<T>`), not a polling
  `run_if(Without<Marker>)` system — the latter re-scans the whole scene every
  frame if any code path forgets to insert the marker. If you must poll, mark
  **every** examined entity, including on `else { continue }` exits.

A `run_if`-gated system that still appears in a steady-state profile means its
gate isn't closing — that's the bug, not the cost.
