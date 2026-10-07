# 29 — rumoca workarounds — pending upstream fixes

> Status: Active · Audience: contributors compiling Modelica through rumoca
>
> Pinned to rumoca `eaa5291ff610085cfc02f9673fcb393245feaa9b` (v0.9.20).

This file records the owning compiler and solver boundaries for the pinned
Rumoca dependency. Recheck these contracts when updating the pin. Local owner
patches are documented in `third_party/rumoca/README.md`.

---

## 1. `SimulationSession` silently clamps at `SimOptions::t_end`

**Bug.** `step()` / `advance_to()` do `target.min(self.t_end)` and return `Ok`
while the model clock simply *stops*. No error, no warning. `SimOptions::default()`
has `t_end = 1.0`, so any caller that forgets the horizon parks at t=1s and then
reports a frozen model as a successful run. (Upstream considers this deliberate —
`advance_to_clamps_to_sim_options_end_time` in `rumoca-solver-diffsol/src/session.rs`.)

*How it bit us:* a 60-second rocket burn drained exactly 1 second of propellant
(4000 → 3900.1 kg at 100 kg/s) and reported success.

**Ideal upstream fix.** Either return an explicit error/saturation flag when an
advance is clamped, or make the horizon `Option<f64>` so "no ceiling" is
expressible rather than spelled `t_end = u32::MAX`.

**Chokepoints (never build `SimOptions` by hand):**
- batch / offline / Fast-Run → `lunco_modelica_runner::stepper_options_from_bounds(&RunBounds)`
- live co-sim → `lunco_modelica_worker::worker::live_stepper_options()` (sets `t_end = u32::MAX` as an
  explicit "no ceiling" sentinel) via `lunco_modelica_worker::worker::build_stepper()`

**Enforced by** `crates/lunco-modelica-worker/tests/rumoca_chokepoints.rs::sim_options_are_built_only_by_the_canonical_builders`
— scans the worker, execution, runner, and solver source roots (bins included)
and fails on any `SimOptions::default()` / `SimOptions { … }` outside those two
builders.

**Probe / regression guard.** `crates/lunco-modelica-worker/tests/rumoca_api_coverage.rs::simulation_session_clamps_advance_at_t_end`
— it *asserts the clamp exists*. When rumoca removes the clamp this test FAILS,
which is the signal to revisit the `u32::MAX` sentinel.

---

## 2. Declaration-scoped runtime input initialization

The vendored `rumoca-phase-dae` owns external input initialization. It preserves
an external declaration binding as the DAE input's start expression at its full
qualified path. Internal and connected input bindings retain equation ownership.
Native live stepping, browser stepping, reset, and batch simulation all consume
that compiled initialization through the existing solver.

Original source declarations enter `ModelicaCompiler` unchanged. Library source
sets and user overlays share the same DAE phase. Class defaults cannot collide
through short names, and function argument defaults retain their binding semantics.
Invalid declarations fail at compiler admission or solver initialization.

The inline compiler test `runtime_input_defaults_follow_declaration_scope`
checks class and instance ownership, inheritance, internal equations, and invalid
bindings. The production `modelica_scoped_input_defaults.rhai` gate checks actual
solver observations, explicit override, reset, and batch execution.

---

## 3. Duplicate-class merge failure across two URIs

**Bug.** Registering the same package under two document URIs in one
`rumoca_compile::Session` fails the merge/resolve pass:

```
Duplicate class 'P.M' found in 'b.mo' with non-identical definition
```

…and it says *non-identical* **even when the two sources are byte-identical**,
which strongly suggests the comparison includes spans / source ids rather than
comparing structure only.

*Why it matters:* the same model open in two tabs, or a restored session plus a
shared copy, would break every compile.

**Workaround.** Every compile is made hermetic: `ModelicaCompiler::compile_str`
evicts all other user docs from the shared session first
(`evict_user_docs_except` + `seated_user_uris`, `lunco-modelica-compiler/src/lib.rs`).

**Ideal upstream fix.** Compare class definitions structurally (ignoring spans /
source ids), and accept an identical redefinition instead of erroring.

**Enforced by** `crates/lunco-modelica-worker/tests/rumoca_chokepoints.rs::user_source_is_seated_only_through_the_strip_chokepoint`
— it pins the number of sites that seat documents into the compile session, so a
new un-evicted seat can't be added silently.

> **Local ownership guard:** `ModelicaCompiler::load_source_root_in_memory`
> (`lunco-modelica-compiler/src/lib.rs`, called from the `LoadSourceRoot` command on both worker twins)
> intentionally keeps durable source-root documents outside
> `seated_user_uris`. It now records the authored top-level namespaces from
> every successfully parsed document in `installed_roots`, independent of the
> transport id (`twin:school`, for example). A later `compile_str` of a
> `within P;` member therefore resolves the already-seated root and never adds
> the same class under a second URI. The regression is
> `source_root_smoke::source_root_namespace_owns_later_package_member_compiles`.

**Probe — must go at a RAW `rumoca_compile::Session`.** Probing through
`ModelicaCompiler::compile_str` is worthless: it evicts first, so it tests the
workaround, not the bug. (This tripped me up once already.)

```rust
let mut s = rumoca_compile::Session::new(rumoca_compile::SessionConfig::default());
s.add_document("a.mo", src).unwrap();
s.add_document("b.mo", src).unwrap();   // same source
s.compile_model("P.M")                   // Err(Duplicate class …) today
```

---

## 4. `to_modelica()` mangles multi-modifier declarations

**Bug.** `StoredDefinition::to_modelica()` loses data on a declaration that has
several modifiers plus a binding, and is not even idempotent:

```
in:     parameter Real k(start = 1.0, fixed = true) = 0.5;
pass 1: parameter Real k(fixed = true) = 1.0;   // dropped `start`; 0.5 → 1.0
pass 2: parameter Real k(fixed = true) = 0.0;   // 1.0 → 0.0
```

This is entirely in rumoca's parse/emit (the test only calls `parse_to_ast` +
`to_modelica`).

**Workaround.** **The splice engine** (`ast_mut/edit.rs`). Policy: **an edit may
only touch the bytes it means to change.** Existing nodes keep their original
source bytes; only genuinely NEW nodes are rendered, by `pretty.rs` (our own
subset emitter). rumoca's emitter is never used to produce source.

**Chokepoint.** `ast_mut::class_patch` / `ast_mut::document_patch` (`ast_mut/mod.rs`),
reached from every one of the ~25 structured `ModelicaOp` arms in
`document/apply.rs`. Each mutation records byte-level `Splice`s against the
original source; `Edit::into_patch` merges them into one patch whose *gaps* are
copied from the source verbatim. A sibling declaration inside the patched range
survives byte-for-byte because no splice ever claims it.

Values come from AST spans (exact — `binding` → `2.0`, a modifier value → its
bytes); structural anchors (where does this declaration end, where does a new
equation go) come from a lexically-aware scanner in `ast_mut/text.rs` that skips
strings and comments.

> **This was an open bypass until it was closed.** `regenerate_class_patch` used
> to rebuild the *whole class* with `to_modelica()` and splice it over the
> original bytes. Dragging one icon on the canvas therefore re-emitted every
> declaration in that class through the broken emitter: an untouched
> `parameter Real m(start = 1, min = 0, unit = "kg") = 5;` came back as
> `m(min = 0, unit = "kg") = 1`, and `parameter Real k = 2.0;` as `k = 0.0`.
> Comments were dropped too. Silent corruption of the user's model from a mouse
> drag — that is what the splice engine exists to prevent.

**Enforcement.** `crates/lunco-modelica-worker/tests/rumoca_chokepoints.rs::source_is_never_regenerated_through_the_rumoca_emitter`
fails on any `.to_modelica(` in `src/`.
`tests/ast_mut_preserves_untouched_source.rs` asserts, per op, that every line the
op did not target is byte-identical afterwards.

**Ideal upstream fix.** Make `to_modelica()` a faithful, idempotent round-trip for
multi-modifier + binding declarations. That would let `pretty.rs` (~900 LOC) go,
but **not** the splice engine: even a perfect emitter can't preserve comments or
formatting, so re-emitting a class the user authored stays the wrong move.

**Verification.** The maintained coverage is the production splice suite in
`tests/ast_mut_preserves_untouched_source.rs` plus the source chokepoint above.
The former upstream-emitter probe was removed because the application never
uses that emitter and the ignored test did not protect a live contract.

---

## 5. Batch solver floods samples at event crossings

**Bug.** rumoca's batch solver honours `opts.dt` for the output grid but ALSO
records an extra sample at every root/event crossing. An event-heavy model
returned ~5M samples for a requested 1.1k-point grid (~4 GB across 75 vars) and
OOM-killed the wasm worker outright.

**Workaround.** `lunco_modelica_runner::batch_keep_indices` decimates the returned
samples back onto the requested grid.

**Ideal upstream fix.** Keep event samples out of the returned series (or put them
behind an opt-in flag) so the output grid is exactly what was asked for.

---

## 6. Conditional algebraics reconstruct as 0

**Bug.** rumoca's elimination reconstructor evaluates algebraics behind a
conditional as `0`. For the bundled RocketEngine,
`m_dot = if m_prop > 0 and throttle > 0.01 then m_dot_max * throttle else 0`
reads **0 at full throttle**, which zeroes `thrust` and `p_chamber` with it. Every
algebraic observable behind an `if` is dead — it reports a plausible-looking 0
rather than failing.

**Resolution in shipped models.** Continuous algebraic observables use explicit
`max`/`min` clamps. Contact and mission predicates use an authored transition
band: the gate is fully true at its inclusive threshold and reaches zero one
band beyond it. The band is part of the controller input contract, so the model
does not hide a solver-specific fallback or rely on a conditional expression.
The shipped models must remain free of equation-level `if`/`when`; the validator
and the modelica lint policy enforce that rule.

**Probe.** `assets/scenes/tests/rocket_engine_observables.usda` with
`assets/scenarios/tests/rocket_engine_observables.rhai`. The production scene
runner compiles the shipped model through its USD program boundary and Rhai
asserts the public observable values, so this check covers source resolution,
worker lifecycle, output collection, and the runtime contract together.

---

## 7. Connect-equation annotations dropped at parse

**Bug.** `Equation::Connect` carries no `annotation` field, so
`connect(...) annotation(Line(points={...}))` waypoints never reach the AST.
Diagram connection routing can't be *read back* from a parsed model.

**Read-side projection.** Rumoca still does not expose the annotation on
`Equation::Connect`, so `lunco-modelica-index::annotation_source` locates the authored connect
statement from the equation span, parses its standard annotation expression
through the normal Modelica parser, and sends the typed `LineRoute` to both
`ModelicaIndex` and the canvas projection. The source remains authoritative and
there is one shared reader; no UI-only route cache or second annotation grammar
is involved.

**Write side: fixed by the splice engine.** `set_connection_line` and
`set_connection_line_style` used to be **silent no-ops** — with no AST field to
mutate they validated the connection, changed nothing, and still triggered a
whole-class re-emit (see §4). The canvas let you drag a connection line, reported
success, and wrote nothing but corruption.

They now splice the `annotation(Line(...))` in the *source text*, where the
annotation plainly is, so routing and styling work without waiting on upstream.
Fields the caller didn't name are left as authored: re-routing a line keeps a
hand-written `color=`/`thickness=` on the same `Line`.

**Probe.** `index.rs::tests::rebuild_extracts_connect_annotation_waypoints` —
asserts that the authored route is present in the index even while Rumoca's AST
continues to omit the connect annotation.

---

## 8. `SimulationSession` is `!Send`

**Bug (arguably by design).** The session holds `Rc<RefCell<…>>` and non-`Send`
closures, so it cannot cross threads.

**Consequence.** The entire off-thread worker architecture exists to contain it:
a dedicated OS thread natively and a second wasm instance in the browser. The
worker owns the session on both targets; no `Send`/`Sync` assertion or
main-thread simulation path is used. This is the platform boundary, not a bug
to chase.

**Probe.** `fn assert_send<T: Send>() {} assert_send::<rumoca_sim::SimulationSession>();`
— fails at compile time today.

---

## Bump checklist

On every rumoca bump, in this order:

1. `cargo update -p rumoca-compile` (all rumoca crates share one git source, so
   this moves them together).
2. Re-run the probes above; delete any workaround whose probe went green.
3. Bump `EXPECTED_RUMOCA_ARTIFACT_TAG` in `lunco-assets-runtime/src/library.rs` — the bincode'd
   `StoredDefinition` layout is version-sensitive and a stale bundle decodes to
   garbage.
4. Remove `.cache/lunco/library/parsed-library.bin` and run
   `cargo run --release -p lunco-modelica-assets --bin modelica_library_indexer -- --warm`.
5. `cargo test --workspace` **and** `cargo test -p lunco-modelica-ui -- --ignored`
   (the ignored set is where the upstream-bug pins live — that's how the 0.9.20
   bump revealed 7 fixed bugs).
