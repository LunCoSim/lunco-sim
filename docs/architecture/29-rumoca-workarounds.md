# rumoca workarounds — pending upstream fixes

Status: **rumoca `main` @ `e6884d03` (v0.9.20)**, verified 2026-07-14.

Each entry below is a bug in rumoca that we work around downstream. Every one was
**re-probed at the 0.9.20 bump** and is still real — none of this is cargo-culted
from an older version.

The point of this file is twofold:

1. **These belong upstream.** Each entry carries the exact probe that tells you
   whether rumoca has fixed it, and exactly what we get to delete when it has.
   Run the probes at every rumoca bump; delete the workaround the moment its
   probe goes green.
2. **A workaround only works if nothing bypasses it.** Each entry names its
   single chokepoint. Anything reaching the raw rumoca API directly re-opens the
   bug silently, so new code must route through the chokepoint — never around it.

> Fixing these in rumoca itself is the preferred end state; this file is the
> to-do list for that work, not a defence of the workarounds.

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
- batch / offline / Fast-Run → `experiments_runner::stepper_options_from_bounds(&RunBounds)`
- live co-sim → `worker::live_stepper_options()` (sets `t_end = u32::MAX` as an
  explicit "no ceiling" sentinel) via `worker::build_stepper()`

**Enforced by** `tests/rumoca_chokepoints.rs::sim_options_are_built_only_by_the_canonical_builders`
— scans `src/` (bins included) and fails on any `SimOptions::default()` /
`SimOptions { … }` outside those two builders.

**Probe / regression guard.** `tests/rumoca_api_coverage.rs::simulation_session_clamps_advance_at_t_end`
— it *asserts the clamp exists*. When rumoca removes the clamp this test FAILS,
which is the signal to revisit the `u32::MAX` sentinel.

---

## 2. A bound `input` is demoted to an algebraic

**Bug.** `input Real g = 9.81` (an input with a default) is demoted to an
algebraic variable, so it never appears in `SimulationSession::input_names()` and
`set_input("g", …)` fails. rumoca offers no compile-time "runtime override" API,
so the only lever is the source text.

**Workaround.** `ast_extract::strip_input_defaults()` blanks the `= <expr>` bytes
(length-preserving, so diagnostic offsets still map to the editor buffer) and
returns the defaults separately, to be re-seeded via `set_input`.

**Ideal upstream fix.** Keep a bound top-level `input` as an input, treating the
binding as its default value (MLS §4.4.1 reading), or expose a parameter/input
override API on the compiled DAE so no source rewriting is needed.

**Chokepoint.** `ModelicaCompiler::seat_user_source` (`lunco-modelica/src/lib.rs`)
— the single place user model text enters the compile session. Both
`compile_str` and `compile_str_multi` go through it, so **the strip happens
inside the compiler and no caller can forget it**.

It did not always live there, and the cost of that was steep: an audit at the
0.9.20 bump found the strip missing on the *entire* experiments/FastRun surface
(native `experiments_runner.rs` **and** the wasm `lunica_worker.rs` twin), on the
worker's disk-fallback compile, and in `modelica_tester` — i.e. the sweep feature
whose whole purpose is overriding inputs was silently demoting every bound input
it swept. Moving the strip into the chokepoint fixed all four at once. Don't move
it back out.

The strip is idempotent (it blanks the binding bytes in place, so a second pass
finds nothing), so callers that also need the defaults map — the worker, to
re-seed values via `set_input` — can still call `strip_input_defaults` themselves.

**Enforced by** `tests/rumoca_chokepoints.rs::user_source_is_seated_only_through_the_strip_chokepoint`
(fails if a new site seats documents into the compile session directly) and
`tests/rumoca_api_coverage.rs::compile_str_keeps_bound_input_as_runtime_slot`
(feeds `compile_str` RAW source and asserts `g` survives as a runtime slot).

**Probe.** Compile `model M input Real g = 9.81; ... end M;` **unstripped** and
read `session.input_names()`. Today: `[]` (empty). When it lists `g`, delete
`strip_input_defaults` and all its call sites.

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
(`evict_user_docs_except` + `seated_user_uris`, `lunco-modelica/src/lib.rs`).

**Ideal upstream fix.** Compare class definitions structurally (ignoring spans /
source ids), and accept an identical redefinition instead of erroring.

**Enforced by** `tests/rumoca_chokepoints.rs::user_source_is_seated_only_through_the_strip_chokepoint`
— it pins the number of sites that seat documents into the compile session, so a
new un-evicted seat can't be added silently.

> **Known hole (latent):** `ModelicaCompiler::load_source_root_in_memory`
> (`lib.rs`, called from the `LoadSourceRoot` command on both worker twins) calls
> `session.add_document` directly and does NOT record the URI in
> `seated_user_uris` — so those docs are never evictable. A library root seated
> there holding package `P`, plus a later `compile_str` of a `P`-member file under
> a different URI, reproduces exactly the duplicate-class failure this workaround
> exists to prevent. Durable-root semantics make it *intended*; nothing enforces
> the disjointness.

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

**Enforcement.** `tests/rumoca_chokepoints.rs::source_is_never_regenerated_through_the_rumoca_emitter`
fails on any `.to_modelica(` in `src/`.
`tests/ast_mut_preserves_untouched_source.rs` asserts, per op, that every line the
op did not target is byte-identical afterwards.

**Ideal upstream fix.** Make `to_modelica()` a faithful, idempotent round-trip for
multi-modifier + binding declarations. That would let `pretty.rs` (~900 LOC) go,
but **not** the splice engine: even a perfect emitter can't preserve comments or
formatting, so re-emitting a class the user authored stays the wrong move.

**Probe.** `tests/ast_roundtrip.rs::component_with_modification` (currently
`#[ignore]`d with this exact reason). Un-ignore when it passes.

*Note:* three sibling round-trip bugs (`redeclare_short_class`,
`conditional_component`, `inner_outer_prefix`) WERE fixed in 0.9.20 and are now
un-ignored and guarding.

---

## 5. Batch solver floods samples at event crossings

**Bug.** rumoca's batch solver honours `opts.dt` for the output grid but ALSO
records an extra sample at every root/event crossing. An event-heavy model
returned ~5M samples for a requested 1.1k-point grid (~4 GB across 75 vars) and
OOM-killed the wasm worker outright.

**Workaround.** `experiments_runner::batch_keep_indices` decimates the returned
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

**Workaround.** None that works. The model's own comment says the Boolean was
inlined specifically to dodge this, and that doesn't help either.

**Probe.** `lib.rs::observables_smoke::rocket_engine_observables_round_trip`
(`#[ignore]`d, `TODO(rumoca-observables)`).

---

## 7. Connect-equation annotations dropped at parse

**Bug.** `Equation::Connect` carries no `annotation` field, so
`connect(...) annotation(Line(points={...}))` waypoints never reach the AST.
Diagram connection routing can't be *read back* from a parsed model.

**Workaround (read side).** `rebuild_from_ast` returns empty waypoints; the
diagram falls back to straight lines.

**Write side: fixed by the splice engine.** `set_connection_line` and
`set_connection_line_style` used to be **silent no-ops** — with no AST field to
mutate they validated the connection, changed nothing, and still triggered a
whole-class re-emit (see §4). The canvas let you drag a connection line, reported
success, and wrote nothing but corruption.

They now splice the `annotation(Line(...))` in the *source text*, where the
annotation plainly is, so routing and styling work without waiting on upstream.
Fields the caller didn't name are left as authored: re-routing a line keeps a
hand-written `color=`/`thickness=` on the same `Line`.

**Probe.** `index.rs::tests::rebuild_extracts_connect_annotation_waypoints`
(`#[ignore]`d) — un-ignore when the AST carries the annotation, which would let
the *read* path recover routing too.

---

## 8. `SimulationSession` is `!Send`

**Bug (arguably by design).** The session holds `Rc<RefCell<…>>` and non-`Send`
closures, so it cannot cross threads.

**Consequence.** The entire off-thread worker architecture exists to contain it:
a dedicated OS thread natively, a second wasm instance in the browser, plus an
`unsafe impl Send/Sync for InlineWorker`. This is the single most expensive
workaround in the crate and the least likely to be fixed — treat it as the
platform, not a bug to chase.

**Probe.** `fn assert_send<T: Send>() {} assert_send::<rumoca_sim::SimulationSession>();`
— fails at compile time today.

---

## Bump checklist

On every rumoca bump, in this order:

1. `cargo update -p rumoca-compile` (all rumoca crates share one git source, so
   this moves them together).
2. Re-run the probes above; delete any workaround whose probe went green.
3. Bump `EXPECTED_RUMOCA_ARTIFACT_TAG` in `lunco-assets/src/msl.rs` — the bincode'd
   `StoredDefinition` layout is version-sensitive and a stale bundle decodes to
   garbage.
4. `rm .cache/msl/parsed-msl.bin && cargo run --release --bin msl_indexer -- --warm`.
5. `cargo test --workspace` **and** `cargo test -p lunco-modelica -- --ignored`
   (the ignored set is where the upstream-bug pins live — that's how the 0.9.20
   bump revealed 7 fixed bugs).
