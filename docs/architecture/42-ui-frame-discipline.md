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

The render camera binder applies the same rule to Bevy's clustered-light
infrastructure. `Camera3d` requires a `Clusters` component, but directional
lights do not use it and Bevy's default allocates a 4,096-cell grid even when
the world has no point lights, spot lights, light probes, or clustered decals.
Automatic render-camera bindings select Bevy's `ClusterConfig::Single` in that
topology and lifecycle observers restore the normal Bevy configuration when a
clusterable object appears. The camera reconciler also waits for Bevy's
positive `Clusters` dimensions before activating a newly projected window
camera, so GPU extraction cannot receive an invalid zero-sized view. An
explicit `ClusterConfig` remains authoritative. This is a renderer-owned
topology/readiness decision; it is not an Apollo scene or name heuristic and
it does not add an alternate lighting path.

The empty scene-root mount path is resolved against the same live composed
stage, through the USD boundary's shared `defaultPrim` resolver. Visual and
celestial projection therefore read the identical concrete root even when
their systems observe the load in the same frame; a deferred visual write
cannot cause a site anchor to be skipped permanently.

BigSpace propagation follows the same schedule-boundary rule. The application
admits `LocalFloatingOrigins`, `PropagateHighPrecision`, and
`PropagateLowPrecision` only when their conservative ECS invalidation inputs
change, while BigSpace remains the authoritative owner of propagation and
per-entity pruning. The gates include origin-cell and hierarchy changes,
spatial transform/cell/grid changes, component additions/removals, and
stationary-entity initialization. They do not gate the normal large-transform
recentering path or add a second transform cache; a stable frame therefore
avoids BigSpace's clean-frame worker setup without weakening transform
correctness.

Generated Modelica projection follows the same frame-discipline contract. Its
shared USD root predicate runs before synthesizer selection, and the projector
uses Bevy identity change detection to reprocess only prims whose path or
identity changed. USD wiring or member-source invalidation explicitly requests
the broader root pass; an unrelated descendant identity does not.

The authored camera-contract admission check follows the same boundary. It
validates roots, camera-track plans, camera identities, and ancestry only after
the scene mount, USD revision, camera/track lifecycle, or required-host setting
changes. Its verdict is not treated as an input, so publishing diagnostics does
not reopen the structural scan on every frame.

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
- The Workbench keeps its immutable theme snapshot and derived egui visuals
  behind the theme revision. A stable frame reuses that snapshot and does not
  reapply context-wide visuals. The runtime-UI render acknowledgement follows
  the same contract: it traverses extracted UI nodes only when an authored,
  visible surface is required by an active recording contract.
- The Modelica editor index is an asset task, not a boot-time render dependency.
  Native MSL source readiness installs the source root immediately; the large
  generated palette index is decoded off-thread and publishes one readiness
  event for the browser to enrich its already-available bundled-model view.
- Asset catalog enumeration is an async projection of the shared discovery
  owner. USD, WGSL, Modelica, and Python listings are collected in one task and
  published by their consumers; startup and Twin lifecycle observers do not
  walk filesystem roots on the UI schedule. Each USD listing owns a generation
  and a complete read set: a newer Twin/manifest snapshot reopens path
  admission, while completions from older snapshots are discarded at the
  generation boundary.
- The shared Modelica engine sync is revision- and completion-driven. Its
  document-generation cursor scans the registry only after a document change,
  an async parse/library completion, or an expired edit-debounce deadline;
  completion workers use a mutex-ordered wake latch, so bounded drains do not
  lose queued work and idle frames do not poll the engine.
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
- **Generated Modelica source metadata** is invalidated by the generated USD
  source component and by explicit document-link/removal dirtiness. The
  publisher does not treat `ModelicaModel` output/time updates as source
  metadata changes; solver state remains in the Modelica owner while the
  generated source registry stays stable between projection events.
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

The same rule applies below the UI boundary. The Modelica engine-sync pass is
woken by the document registry revision and still compares document generations
before dispatching work. Modelica and physics telemetry retain producer-owned
model/fixed-time cursors while the shared signal registry remains the channel
authority. Autopilot target paths are cached per behavior entity and invalidated
by authored XML or active-frame ancestry changes. Celestial terrain curvature is
reconciled only when its authoritative inputs change, and globe LOD caches the
pure desired leaf selection separately from readiness and bounded tile
streaming. These are owner-local cursors, not compatibility stores or alternate
sources of truth.

The engine exposure producer has one shared 20 Hz cadence gate for its change
detector and publisher. The first publication is immediate; subsequent stable
frames do not even enter the publisher's large query set. `ExposureRefresh`
records separate invalidation domains for the driven vessel, authored control
cards, schema, celestial capability, and progress overlays. A rover motion
therefore does not re-read static schema or celestial USD data. Authored
control-root topology is resolved through the existing `UsdStageRevision` and
cached until that revision changes; it is not discovered by rescanning every
USD prim on every telemetry publication.

State mirrors must keep the live progress entry as the authoritative UI state
for an in-flight operation and publish one current terminal event when that
entry completes. They must not leave a historical "started" event as the only
visible result: the status bar falls back to discrete history after progress is
removed, which can make completed terrain appear stuck at an old tile count.

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

Trajectory overlays have an additional presentation boundary. Their ephemeris
sampling and mesh rebuild run only for an active `TrajectoryView` (`is_visible &&
user_visible`); a hidden authored orbit must not consume orbital or GPU budget.
The sampled path owns a geometry revision, while the runtime state owns separate
sampling, frame/provider, and presentation revisions. Mesh and fade tasks carry
those revisions and stale results are discarded; empty or failed inputs are
resolved once until an input revision changes. This prevents an unchanged or
paused clock from reallocating and re-uploading trajectory buffers, and prevents
missing frame/data from retrying every frame. An existing active curve also holds
its stamped fade during high-rate Celestial transport, including an independently
scaled Celestial clock. The body and frame poses continue to use the current
epoch. Ephemeris sampling, spline tessellation, and alpha-buffer construction
are compute tasks; the main schedule performs one non-blocking poll and commits
only a current, prepared result. Anchored curve alignment reads the already-
solved tracked/reference frame pose through `lunco_core::coords::pose_in_grid`;
it does not evaluate ephemeris endpoints again for presentation.
