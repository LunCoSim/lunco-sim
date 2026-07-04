# 27 — Simulation Target & Run-Configuration Resolution

> Status: Design · Audience: contributors planning target/run-config resolution (proposal, not implemented)

**Scope:** how LunCoSim decides *which* thing to simulate and *with what bounds*, why the current logic breeds drift bugs, how to make that bug class unrepresentable, and how the same machinery generalizes from Modelica to USD (framed against the FMI / SSP standards).

---

## 1. The problem (the bug class)

The typical resolution bug has the same shape: **one question is answered by N inlined implementations that drift apart.** Three questions, each answered in ≥3 places, each with an independent copy that can (and did) diverge:

| Question | Implementations | Observed drift |
|---|---|---|
| Which class? | `simulation_candidates()` (tier-ranked) **vs** `first_non_pkg` (HashMap order) **vs** `dispatch_experiment`'s own manual candidate list | ranked vs unranked → different class chosen → wrong/short run |
| Which name matches the query? | exact-or-leaf `rsplit('.').next()` idiom copy-pasted in ≥4 sites | one copy did exact-only → short-name `FastRunActiveModel{class:"RoverThermalSystem"}` missed the `experiment(...)` annotation, silently fell back |
| Which bounds / what fallback? | `resolve_setup_bounds` (fallback `t_end = 10.0`) **vs** `dispatch_experiment` (fallback `t_end = 1.0`) | **live divergence**: panel & API show 10 s, FastRun actually runs 1 s |

Initial fixes were **point fixes** (swap `first_non_pkg` → `simulation_candidates()` at two call sites). They removed three divergences but left the structure that breeds them: resolution is computed *inline at each call site*. While that is true, every new call site reinvents the logic and re-introduces drift.

---

## 2. Current system inventory

File references in `crates/lunco-modelica` and `crates/lunco-experiments`:

### 2.1 Class candidates & ranking — `index.rs`
- `ClassKind::is_simulatable()` (`index.rs:281`) → `true` only for `Model | Block | Class`.
- `simulation_candidates() -> Vec<String>` (`index.rs:699`): filters `is_simulatable() && !partial`, ranks by `sim_tier`, then alphabetical. Returns fully-qualified names.
- `simulation_preferred_count() -> usize` (`index.rs:715`): count of classes in the best (lowest) tier. `== 1` → auto-pick; `!= 1` → ambiguous → picker.
- `sim_tier(c, used) -> u8` (`index.rs:758`): **0** = has `experiment(...)` annotation; **1** = top-level (not used as a subcomponent); **2** = used as a subcomponent/helper.

### 2.2 The four resolution paths
1. **`on_compile_model`** (`compile.rs:529`) — class precedence: `explicit > drilled > picker(if ambiguous) > detected[0]`. Does not resolve bounds (compile only).
2. **`dispatch_experiment`** (`compile.rs:1252`) — class precedence: `explicit > drilled > picker > sole`; **builds its own candidate list by raw `index.classes` iteration** (does not call `simulation_candidates()`). Bounds composed as a 4-layer overwrite: `fallback(t_end=1.0) → annotation → draft → cmd_override` (`compile.rs:1386`+, fallback literal at `compile.rs:1401`).
3. **`render_setup_section`** (`experiments.rs:804`) — class: `drilled > simulation_candidates()[0]`; bounds via `resolve_setup_bounds`.
4. **`QueryExperimentBounds`** (`api_queries.rs:410`) — lists all non-package classes; per class reports `resolved_bounds` (via `resolve_setup_bounds`) + recomputes a `source` label (`"draft_override" | "runner_cache" | "annotation" | "fallback_10s"`).

### 2.3 Bounds resolution — `compile.rs`
- `resolve_setup_bounds() -> RunBounds` (`compile.rs:1221`): precedence `draft override → runner cache → annotation → fallback(t_end=10.0)` (fallback literal at `compile.rs:1244`).
- `bounds_from_annotation() -> Option<RunBounds>` (`compile.rs:1191`): looks up class by qualified **or** leaf name; requires `experiment.stop_time = Some(_)`; maps `start_time → t_start` (default 0.0), `stop_time → t_end`, `interval(>0) → dt`, `tolerance → tolerance`; `solver`/`h0` always `None`.

### 2.4 Types — `lunco-experiments/src/lib.rs`
- `ModelRef(String)` (`:65`) — opaque qualified class name; the crate does **not** depend on `lunco-modelica`.
- `RunBounds { t_start, t_end, dt: Option, tolerance: Option, solver: Option<String>, h0: Option }` (`:97`).
- `ExperimentRunner { run_fast(&Experiment) -> RunHandle; default_bounds(&ModelRef) -> Option<RunBounds> }` (`:491`) — already backend-agnostic; one impl (`ModelicaRunner`, `experiments_runner.rs:289`).
- `ExperimentRegistry` — keyed `(TwinId, ModelRef)`; Modelica-specific in usage.
- `ExperimentDrafts` — `(DocumentId, ModelRef) → ExperimentDraft { bounds_override: Option<RunBounds>, .. }`.

### 2.5 "Drilled / focused class"
- `ModelTabs::drilled_class_for_doc(doc) -> Option<String>` (`model_view/tabs.rs:205`): the class the user navigated into on a tab. Preferred over auto-ranking everywhere. This is the *one legitimate UI signal* that all paths agree on.

---

## 3. Divergence summary (what actually differs)

| Path | Class algorithm | Bounds fallback `t_end` | Bounds precedence mechanism |
|---|---|---|---|
| `on_compile_model` | ranked candidates + picker | — | — |
| `dispatch_experiment` | **manual unranked list** | **1.0 s** | overwrite: fallback→annotation→draft→cmd |
| `render_setup_section` | `simulation_candidates()` | 10.0 s | first-Some: draft→cache→annotation→fallback |
| `QueryExperimentBounds` | all non-package | 10.0 s | + independently recomputes provenance label |

Two implementations of "no bounds known" (1.0 vs 10.0). Two implementations of precedence (overwrite vs first-Some) — these can disagree on **partial overrides** (e.g. a draft that sets only `t_end`: where does `t_start` come from? answered differently by each). Provenance ("where did this value come from") is computed a third time, separately, in the API query.

---

## 4. Design principle: make the bug class unrepresentable

**Target invariant:** *exactly one pure function maps `(index, query, drilled, drafts, cache) → Resolution`; it returns ambiguity and provenance as data; the fallback is a single named constant; and the building blocks it uses are not exported so no call site can reinvent them.*

Every divergence bug requires two implementations to disagree. With one implementation, disagreement is structurally impossible.

### 4.1 One resolver returning a sum type — ambiguity is a value, not a side effect
```rust
pub enum Resolution {
    Resolved { target: SimTarget, bounds: RunBounds, why: Provenance },
    Ambiguous { candidates: Vec<SimTarget> }, // caller opens picker; resolver never does
    Nothing,                                  // nothing simulatable
}
pub struct Provenance { pub class_from: ClassSource, pub bounds_from: BoundsSource }
```
The picker modal becomes a single UI reaction to `Ambiguous`. This removes the duplicated picker-trigger logic in `on_compile_model` and `dispatch_experiment`, and makes resolution pure and unit-testable (no `&World`, no egui).

### 4.2 Newtype the class name → name-matching cannot drift
The `rsplit('.')` bug existed because a class name is a bare `String` matched ad-hoc at four sites.
```rust
pub struct QualifiedClass(String);          // canonical, fully-qualified
impl QualifiedClass { pub fn leaf(&self) -> &str { self.0.rsplit('.').next().unwrap_or(&self.0) } }

pub enum ClassQuery { Qualified(String), Leaf(String) } // what the caller passed
fn resolve_query(q: &ClassQuery, cands: &[QualifiedClass]) -> Option<QualifiedClass>; // the ONLY matcher
```
No call site does its own `rsplit`. The exact-only bug is uncompilable because there is no second matcher to get wrong.

### 4.3 One bounds default
```rust
impl Default for RunBounds { fn default() -> Self { Self { t_end: 10.0, /* … */ } } }
```
Delete the `1.0` literal at `compile.rs:1401`. Both paths call `RunBounds::default()`. The two-constant divergence cannot recur because there is one constant.

### 4.4 Bounds precedence as one fold that returns its source
```rust
fn resolve_bounds(layers: BoundsLayers) -> (RunBounds, BoundsSource);
// layers: { draft: Option, runner_cache: Option, annotation: Option }
```
Partial-override semantics defined exactly once. Returns provenance so `QueryExperimentBounds` stops recomputing the source label — single source of truth for "where did this come from."

### 4.5 Make the parts private
Delete/privatize `first_non_pkg`, the manual candidate building, and the inline matchers. The type system then offers only `resolve(...)`. A new call site cannot reinvent the logic because the parts are unreachable.

### 4.6 The unifying frame — class and bounds are one fold, run twice

§4.1–4.5 are not five separate fixes; they compose into a single pipeline. The core realization: **"which class" and "which bounds" are not two problems — they are the same operation applied to two value types, and bounds is the *downstream* stage of class.**

**Coupled, not parallel.** Bounds depend on the class: the `experiment(...)` annotation lives on the class, the draft is keyed `(doc, class)`, the runner cache is keyed by class. You cannot resolve bounds "for a doc" — only for a *resolved* class. Today `resolve_setup_bounds(world, doc, model_ref)` already takes a `model_ref`, meaning the caller resolved the class by some *other* rule first — which is exactly why bounds end up correct for the wrong class. Unification = class resolution is the first stage of the same call, so bounds are always for the class the resolver itself picked.

**The same fold both times.** Class and each bound field resolve down the *identical* source ladder — first present wins, record which layer won:

| Layer | Class | a bound field (e.g. `t_end`) |
|---|---|---|
| **Request** (API/cmd) | explicit `class:` param | cmd override |
| **Draft** (user UI) | drilled class | draft field |
| **Cache** (compiled) | — | runner-cache field |
| **Declared** (the model) | top-tier ranked candidate | annotation `stop_time` |
| **Default** (hard) | → `Ambiguous` / `Nothing` | `RunBounds::default()` |

```rust
fn pick<T>(ladder: [(Source, Option<T>)]) -> Resolved<T>; // first Some wins, carries Source
enum Source { Request, Draft, Cache, Declared, Default }
```

Class is just *the first field resolved*, and the **only** field that can be `Ambiguous` (multiple candidates tie at the top tier; no hard default). Every bound field always resolves because `Default` is total. This collapses the two precedence *mechanisms* (overwrite-fold in `dispatch_experiment` vs first-Some in `resolve_setup_bounds`) into one combinator.

The whole resolver is then:
```rust
fn resolve(req: ResolveRequest, ctx: &ResolveCtx) -> Resolution
struct ResolveRequest {            // the ONLY per-call-site variation
    explicit: Option<ClassQuery>,
    drilled:  Option<QualifiedClass>,
    overrides: BoundsOverride,
}
```
Internally: `pick` the class over one borrowed `ResolveCtx` (index, drafts, cache, drilled — **no `&World`**); if `Ambiguous`, return now (bounds are unknowable without a class); else `pick` each bound field against the sources **keyed by that class**. One context walk, not two. The four call sites collapse to *fill `ResolveRequest` differently → call `resolve` → react*, and `QueryExperimentBounds` *reads* `why` instead of recomputing the source label.

**`RunBounds` becomes the output of resolution, not a value juggled per-site** — with one `Default` and per-field provenance. The Declared layer is already abstracted as `ExperimentRunner::default_bounds` / `TargetSource::declared_bounds` (Modelica annotation today, USD scene metadata / FMU `DefaultExperiment` tomorrow), so the same fold runs across backends — which is why this unification *is* the §5 USD generalization, not a separate effort.

---

## 5. Generalization to USD — framed against FMI & SSP

### 5.1 What the standards say (verified)
- **FMI 3.0** (Functional Mock-up Interface, Modelica Association) standardizes a *single component* — an FMU: a black box with declared inputs/outputs/parameters and a step function (`fmi3DoStep`). Its `modelDescription.xml` carries an optional **`DefaultExperiment`** element with `startTime / stopTime / tolerance / stepSize` — a 1:1 match for `RunBounds` (`h0` ≈ `stepSize`). ([fmi-standard.org/docs/3.0](https://fmi-standard.org/docs/3.0/))
  - **Correction worth internalizing:** the **co-simulation *master algorithm* is explicitly NOT part of the FMI standard** — FMI standardizes only the component interface and leaves the master to the tool. So the LunCoSim cosim master loop is *not* a fork-gone-wrong; owning the master is exactly what FMI expects. **Only the component boundary should align to FMI — the loop is ours to own.**
- **SSP 2.0** (System Structure & Parameterization, same body; released Dec 2024 / Jan 2025) standardizes the *system around the components* — which components exist, how their connectors are wired, and parameter values, in a `.ssp` container (`SystemStructure.ssd` + `.ssv` parameter sets + `.ssm` mappings). It is FMI's companion: "FMI exchanges individual models, SSP exchanges composite systems." ([ssp-standard.org/docs/2.0](https://ssp-standard.org/docs/2.0/))
  - **Nuance that matters for us:** SSP 2.0 adds **Modelica-based components and both *causal and acausal* connection semantics** (on top of FMI 3.0). LunCoSim's `SimConnection` is **causal-only** (output→input f64). Modelica connectors are acausal — so if we ever ingest Modelica-as-component (not just Modelica-compiled-to-scalar-ports), the causal-only wire model is a real limitation SSP 2.0 already solved.

### 5.2 The realization: LunCoSim is a private fork of FMI + SSP
| Standard | LunCoSim equivalent | File |
|---|---|---|
| FMU (component) | `SimComponent { model_name, inputs, outputs, parameters, .. }` | `lunco-cosim/src/component.rs` |
| FMU `DefaultExperiment` | `RunBounds` (`h0` ≈ `stepSize`) + `experiment(...)` annotation | `lunco-experiments/src/lib.rs:97` |
| FMI master algorithm | cosim master loop (`sync_outputs → propagate → sync_inputs → step`) | `lunco-cosim/src/lib.rs`, see [22-domain-cosim](22-domain-cosim.md) |
| SSP System | USD Stage / active `scene.usda` | [21-domain-usd](21-domain-usd.md) |
| SSP Component (FMU ref) | USD prim + `lunco:modelicaModel` attr + `lunco-lib://` payload | |
| SSP Connection (+ `factor`/`offset`) | `SimConnection { start/end element+connector, scale }` | `lunco-cosim/src/connection.rs` |
| SSP `.ssv` parameter sets | USD attributes + layer/reference overrides; `Experiment.overrides` | |

**USD already plays SSP's role, and plays it better:** USD composition (layers, references, payloads, overrides) is a superset of SSP's flat `.ssv`/`.ssm`. So the correct mental model is three layers:
- **USD scene** = SSP-equivalent *authoring/storage* (what + how-wired + params),
- **`SimComponent`/`SimConnection`** = the *instantiated runtime* graph (the master's component set),
- **SSP** = relevant only as an *interop wire format* (import/export to FMPy, Twin Builder, Modelon).

### 5.3 The clean target generalization
```rust
pub enum SimTarget {
    Modelica(QualifiedClass), // one component
    Usd(UsdSceneRef),         // a wired system (path + root_prim)
}
pub trait TargetSource {                 // resolution is generic over this
    fn candidates(&self, ctx: &ResolveCtx) -> Vec<SimTarget>;
    fn tier(&self, t: &SimTarget) -> u8;             // Modelica: sim_tier; USD: manifest-explicit-first
    fn default_bounds(&self, t: &SimTarget) -> Option<RunBounds>;
}
```
- `ModelicaSource`: candidates = `simulation_candidates()`, tier = `sim_tier`, bounds = `experiment(...)`.
- `UsdSource`: candidates = scenes (`twin.toml [usd] root`, else `scene.usda`/`main.usda` convention via existing `resolve_root_prim`), tier = explicit-manifest-first, bounds = `.mission.ron`/scene metadata.

The generic `resolve(source, ctx) -> Resolution` (drilled > ranked-first > `Ambiguous` > `Nothing`, plus the §4.4 bounds fold) lives once and is parameterized. **`SimTarget::Usd` = "instantiate this SSP-style system and run the master"; `SimTarget::Modelica` = "run one component."** Same runner trait, same `RunBounds`, same registry. This is the FMI-component / SSP-system division applied to two target kinds — not a novel invention.

---

## 6. What we're missing (gaps vs the standards)

Prioritized by value.

1. **No declared, typed, causal ports.** SSP requires each connector to declare causality (input/output/parameter) and data type. LunCoSim ports are untyped `String` keys in HashMaps, discovered at runtime — which is why a typo in `lunco:simWires` fails silently instead of at author time, and why a wire can't be validated before stepping. **Biggest actionable gap.** USD can carry this as a connector schema on the prim.
2. **Connection transform is half of SSP's.** `SimConnection.scale` = SSP `factor`; there is no `offset` and no unit conversion at the boundary — directly relevant to [41-axes-and-units](41-axes-and-units.md), which is exactly the unit/transform concern SSP folds into the connection.
3. **Run-config is not a first-class object.** Mature tools (Simulink configuration sets **[verify]**) keep solver/time/tolerance settings as named, switchable objects *separate from the model*. LunCoSim conflates target and settings; `ExperimentDrafts` is a half-step. A named `Scenario`/`RunConfig` dissolves the §4.4 precedence mess: bounds live in a Scenario; the annotation is merely the *seed* for a new Scenario, not one of four competing layers. SSP confirms the two-level split (system-level settings + per-FMU `DefaultExperiment`), which matches `twin.toml`/`.mission.ron` vs `experiment(...)`.
4. **Time-only bounds — no stopping conditions.** GMAT and orbital sims terminate on *events* (elapsed time, apoapsis, altitude, contact, fuel depletion) **[verify]**. LunCoSim's domain is lunar/orbital (rover thermal over a lunar day; Abdulezer antipodal hops) — `RunBounds` cannot express "run until sunrise" / "until landing." Leave a `stop_condition` slot in the type.
5. **No solver capability negotiation.** The session's ESDIRK34-fails / BDF-works / tol-must-be-1e-4 trial-and-error happened because stiffness is a *property of the model* that should travel with the target and be negotiated, not guessed. FMI puts suggested tolerance in `DefaultExperiment`; a `TargetSource::solver_hints()` would carry "this thermal model is stiff → BDF + loose tol" and prevent the detour.
6. **No reproducibility provenance on results.** `Provenance` (§4.1) covers resolution-time "which class / which bounds source," but the fully-resolved `SimTarget + RunConfig` should be stored *with each `RunResult`* so any registry entry is re-runnable identically. Currently discarded after a run starts.

---

## 7. Recommended structure & crate placement

Three layers (consistent with the S2005 plan, with one correction).

1. **Pure core — in `lunco-experiments`** (NOT `lunco-modelica`). Correction to the earlier plan: USD cannot depend on `lunco-modelica`, so the shared types must live in the backend-agnostic crate that already owns `ExperimentRunner`/`ModelRef`/`RunBounds`. Put here: `SimTarget`, `Resolution`, `Provenance`, `QualifiedClass`, `ClassQuery` + the one matcher, `RunBounds::default()`, `resolve_bounds` fold, the `TargetSource` trait, and the generic `resolve()`. No Bevy/egui/`&World`. **Unit-tested in isolation.**
2. **Backend resolvers:** `impl TargetSource for ModelicaSource` in `lunco-modelica`; `impl TargetSource for UsdSource` in a usd crate.
3. **Bevy adapter** (per crate): reads index/drafts/cache/drilled from `World`, builds `ResolveCtx`, calls `resolve()`, writes a derived `ExperimentTargets` resource (runtime state, not UI state).
4. **UI:** read-only consumer of `ExperimentTargets`; renders the picker on `Ambiguous`; never computes.

**Wire/API seam decision (open):** keep `ModelRef` as the opaque wire id but give it a **scheme** (`modelica:Foo.Bar`, `usd:scenes/main.usda#/World`) and dispatch on scheme — a tagged union in spirit without churning every API call to carry `enum SimTarget`. Alternative: thread `SimTarget` end-to-end. To be decided before implementation.

---

## 8. Phased plan

- **P0 — kill the live bug (one-liner-ish).** Collapse both fallbacks onto `RunBounds::default()`; delete `compile.rs:1401` `1.0`. Ships immediately; removes the panel-shows-10s / FastRun-does-1s divergence.
- **P1 — pure core (Modelica only).** Land `SimTarget`/`Resolution`/`QualifiedClass`/matcher/`resolve_bounds` in `lunco-experiments` with unit tests. Port the four call sites to `resolve()`. Privatize `first_non_pkg` + inline matchers. This is the consolidation proper.
- **P2 — `TargetSource` trait + `ModelicaSource`.** Parameterize `resolve()`; no behavior change, but the seam for USD exists.
- **P3 — `UsdSource` + registry keyed by `SimTarget`.** USD scenes appear in the experiments panel / `ListRuns` alongside Modelica runs.
- **P4 — gaps:** declared/typed ports (gap #1), connection `offset`+units (#2), `Scenario`/`RunConfig` object (#3), `stop_condition` (#4), solver hints (#5), resolved-config-on-result (#6). Each independently shippable.

---

## 9. Open decisions

1. **Wire identity:** schemed `ModelRef` vs end-to-end `enum SimTarget` (§7).
2. **Auto-ranking vs explicit selection:** OMEdit uses *explicit* target selection and does **not** auto-detect a "main model" (verified). `sim_tier` is a UX convenience that became load-bearing and bred bugs. Decision: keep ranking strictly as a *default highlight*, with `drilled` (explicit) dominating; never let the guess be the only signal a path uses.
3. **How far to chase SSP/FMI interop:** internal alignment only, or actual `.ssp`/FMU import-export? (Affects whether ports/transforms must be spec-faithful.)
4. **Bounds ownership:** fold layers (§4.4) as an interim vs jump straight to a named `Scenario` object (gap #3).

---

## 10. References (internal)
- [13-twin-and-workflow](13-twin-and-workflow.md) — Twin owns documents; Scenario/Run layers.
- [21-domain-usd](21-domain-usd.md) — USD scene resolution, `ActiveStage`, `resolve_root_prim`.
- [22-domain-cosim](22-domain-cosim.md) — master loop, `SimComponent`/`SimConnection`, f64 wires.
- [26-parallel-experiments](26-parallel-experiments.md) — bounded scheduler, `ExperimentRegistry`.
- [41-axes-and-units](41-axes-and-units.md) — unit/transform boundary (relates to gap #2).

## 11. References (external)

Verified standards references:

- FMI 3.0 spec — `DefaultExperiment`, `fmi3DoStep`, black-box FMU, master-not-standardized: <https://fmi-standard.org/docs/3.0/>, <https://github.com/modelica/fmi-standard/blob/main/docs/4_2_co-simulation_api.adoc>
- SSP 2.0 — companion to FMI, Modelica + acausal components: <https://ssp-standard.org/docs/2.0/>, <https://ssp-standard.org/news/2024-12-20-ssp-2-0-release/>
- Modelica `experiment` annotation (`StartTime=0`, `StopTime`, `Interval`, `Tolerance`): <https://specification.modelica.org/master/annotations.html>, <https://build.openmodelica.org/Documentation/ModelicaReference.Annotations.experiment.html>
- OMEdit — explicit model selection, no auto main-model detection: <https://openmodelica.org/doc/OpenModelicaUsersGuide/latest/omedit.html>
- Simulink `Simulink.ConfigSet` — config-only object, separate from model: <https://www.mathworks.com/help/simulink/slref/simulink.configset.html>

**Still open (unverified, not refuted):** GMAT event-based stopping conditions (gap #4); `partial`-class exclusion specifics in OM/Dymola; whether "store resolved run-config with results" (gap #6) is a *named* industry best practice vs a synthesis.
