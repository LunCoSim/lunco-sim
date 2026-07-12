# 37 — Model Synthesis & Multi-Domain Composition

Status: **design / analysis** (2026-07-03). Substrate largely EXISTS; the rover-level synthesizer is the gap.

> Question this answers: *what is the proper way to give a rover an **electrical layer** (and any
> physical-domain layer) that is **built from basic reusable components**, **synthesized as a Modelica
> model at runtime**, yet **hand-editable** — and how does that line up with industry standards, our
> architecture, and our digital-twin goal?*
>
> Short answer: **yes, we can synthesize Modelica at runtime, and most of the machinery already ships.**
> The discipline is a *two-level composition rule* (acausal within a domain, causal across domains) plus
> a *netlist-from-USD synthesizer* that emits real, hand-editable Modelica text. This is precisely how
> the Modelica industry (Dymola / OpenModelica / SystemModeler) already works, and we use real MSL v4.1.0.

Companion to `36-components-and-sky.md` (the multi-layer component model, whose electrical layer
motivated this doc) and `34-scenario-and-multidomain.md` (the sub-prim-per-model decision).
**This doc owns the causal-vs-acausal decision** for the electrical layer; doc 36 §1.3 defers to it.

> **Reframed by `38-domains-as-packages.md`:** the synthesizer registry + hooks of §8 is one slot of a
> general *domain descriptor*; "electrical" is a domain *package* (data + rhai), not a Rust primitive.
> Read doc 38 for the organizing principle (domain-neutral core, generic graph editor, USD organization,
> bidirectional projection). This doc is the *synthesis mechanism*; doc 38 is *what it plugs into*.

---

## 1. The core rule: acausal *within* a domain, causal *across* domains

The single most important design fact, and it is physics, not preference:

- **Within one physical domain (electrical, hydraulic, 1-D mechanical): ONE acausal Modelica model =
  ONE DAE, solved simultaneously.** Kirchhoff's current law (Σ flow = 0 at a node) is an *algebraic
  constraint coupling all elements on that node at the same instant*. You **cannot** co-simulate a
  resistor and a capacitor sharing a node by exchanging scalars with a one-tick lag — that is
  numerically wrong (it breaks the constraint and injects delay into an instantaneous law). Modelica's
  `connect()` with effort/flow semantics exists exactly for this, and **rumoca implements it for real**
  (MLS §9.2: potential→equality, flow→sum-to-zero, stream→inStream) with 128 connect tests
  (`rumoca-phase-flatten/src/connections/`). MSL `Modelica.Electrical.Analog.Basic.{Resistor,Capacitor,
  Inductor,Ground}` are available (real MSL v4.1.0 bundle, ~2670 classes).

- **Across domains (electrical↔thermal↔mechanical↔GNC↔comms): CO-SIMULATION via scalar boundary
  wires.** These couplings are looser and directional (motor electrical power → mechanical torque;
  battery SoC → GNC engine-enable; comms TX state → electrical draw). A one-tick Jacobi lag is
  acceptable and standard (this is what an FMI-CS master does). This is **already what the runtime
  does**: each `lunco:modelicaModel` prim compiles to its own `SimStepper` DAE solver on a background
  thread, and `propagate_connections` (`lunco-cosim/src/systems/propagate.rs:146`) copies
  `src·scale+offset` between them each `FixedUpdate`, summing multiple wires into one input.

So the rule that resolves "how to do the electrical layer":

> **A rover's electrical layer is ONE synthesized `Electrical.mo` (acausal, single DAE), exposed as ONE
> `lunco:modelicaModel` prim, whose BOUNDARY ports (bus voltage, per-load power/current, pack SoC)
> scalar-wire to the other domain prims.** Kirchhoff stays inside one solve; the rest is co-sim.

The rule is **not** "causal p_draw now, acausal later." Acausal is not "later" — it is the
*correct home for the electrical network itself*, and it is available now. Causal scalar ports remain
correct for the **cross-domain boundary** of that network (the loads reported to it, the bus voltage /
SoC it reports out).

---

## 2. Can we synthesize Modelica at runtime? Yes — three tiers, all shipping

The artifact the simulator steps is a `DaeCompilationResult` → `rumoca_sim::SimStepper`, and the entry
point takes a **string**, not a file: `ModelicaCompiler::compile_str(model_name, source: &str,
filename) -> DaeCompilationResult` (`lunco-modelica/src/lib.rs:499`; `compile_str_multi` for cross-doc).
So *any* runtime-generated Modelica text compiles and runs. Three ways to generate that text, in
increasing structure — **all three exist today**:

1. **String-template synthesis (simplest).** Build the `.mo` by concatenating component declaration
   lines + `connect(...)` lines, feed to `compile_str`. The explorer's verdict: for *generating a new
   composed model* this is "the easy direction." Good enough for a v1 netlist emitter.
2. **AST-mutation synthesis (structured, text-canonical).** `lunco-modelica/src/ast_mut/` +
   `pretty.rs` builders: `add_class`, `add_connection`/`remove_connection`/`reverse_connection`
   (`connections.rs:10`), component emitters that produce e.g.
   `Modelica.Electrical.Analog.Basic.Resistor R1(R=100) annotation(...)` (`pretty.rs:112`). Splices new
   nodes into the source buffer **byte-for-byte outside the patched region** — comments and hand-edits
   preserved. This is what the diagram editor uses.
3. **Visual diagram-builder synthesis (already a product feature).** The Modelica workbench canvas:
   drag MSL parts from the palette, wire pins; each drop is an `AddComponent{class:"<qualified MSL
   class>", decl}`, each wire an `AddConnection` → real Modelica text out
   (`document/ops.rs:126` `ModelicaOp` → `ast_mut` → `pretty.rs`). **Dragging `…Analog.Basic.Resistor` +
   `Capacitor` + `Ground` and wiring their pins literally synthesizes an electrical-network `.mo`.**
   `ModelicaOp` derives `Serialize` — the same ops are drivable programmatically.

All three converge on the **same artifact**: real, editable Modelica text with a **lossless
text-canonical round-trip** (source→AST→canvas and canvas→ops→AST→source). A synthesized model is not a
locked black box — it is a `.mo` a human can open and edit, and re-parse without loss.

---

## 3. Where the "basic components" come from — netlist-from-USD

The elegant connection to the component model (doc 36 §1): **each USD component already declares its
electrical contribution**, and the currently-**inert** `rel lunco:epsBus` / `rel lunco:powerInput`
relationships (author-side topology, *no runtime reader today*) **are a netlist**. Synthesis reads the
composed USD graph and emits the rover `Electrical.mo`:

| USD (structure) | → | Modelica (dynamics) |
|---|---|---|
| each electrical component prim (battery, bus, motor, comms `Power` layer) + its `lunco:` params | → | one instance line — a component `.mo` class or an MSL primitive, parameterized |
| each `rel lunco:epsBus` edge (battery↔bus↔loads) | → | one `connect(a.pin, b.pin)` line |
| the whole set | → | one `Electrical.mo` → one DAE (`compile_str` → `SimStepper`) |
| boundary quantities (V_bus, I_load[i], SoC) | → | causal ports scalar-wired to thermal/GNC/comms prims |

This is exactly SPICE netlist → circuit, and it is **the gap** the explorers flagged: per-model diagram
synthesis exists, MSL exists, acausal flatten exists — but *nothing today auto-assembles a rover-level
electrical network from the component library*. The `ModelicaOp` builder + MSL bundle are the precise
primitives to drive that assembly. Building the synthesizer is the new work; every primitive under it
ships.

**Two synthesis stances** (pick per workflow; start with the first):

- **Scaffold-and-own (recommended v1).** A `SynthesizeElectrical` action reads the USD graph once and
  emits `Electrical.mo`; thereafter the `.mo` is user-owned and hand-edited; re-synthesis is explicit
  (and, thanks to text-canonical patching, can targeted-merge rather than clobber). Matches how the
  diagram editor already patches text. Simple, predictable, honors "modify it manually."
- **Live projection (later track).** The USD graph is source-of-truth; `Electrical.mo` is a derived view
  regenerated on graph change, hand-edits flowing back through `ast_mut` ops. This is the diagram↔source
  round-trip generalized to USD↔Modelica — powerful but a bigger commitment (a continuous binding, like
  the doc-35 animate projection). Defer until scaffold-and-own proves the netlist mapping.

---

## 4. Industry-standard alignment

| Standard | What it is | Our status |
|---|---|---|
| **Modelica + MSL** | the industry language for acausal multi-physics; MSL = the standard component library | **Native.** rumoca targets MSL v4.1.0, real `Electrical.Analog`, real `connect()` effort/flow. Drag-parts→`.mo` is the Dymola/OpenModelica/SystemModeler workflow — we have it. |
| **SSP** (System Structure & Parameterization) | XML (`.ssp/.ssd`) wiring of FMUs into a system | **Concept, not format.** `SimConnection` = SSP Connection + `scale`/`offset` = SSP LinearTransformation, but encoded in **USD**, not `.ssp` XML. No `.ssp` parser. USD is our system-structure carrier; `.ssp/.ssd` export is a future interop bridge. |
| **FMI** (Functional Mock-up Interface) | `.fmu` packages for tool-neutral model exchange / co-sim | **Latent.** rumoca *has* `fmi2`/`fmi3` codegen templates — not wired into usd. Path exists to (a) export any synthesized/component model as an FMU for external tools, (b) import external FMUs as co-sim participants. Real future capability, unbuilt in usd. |
| **SysML v2** | system architecture / requirements / structure | **Stubbed** (doc 24). Ideally the "which components, which connections" traces to SysML v2; today USD carries it. |
| **SPICE / netlist** | EE circuit description | The `epsBus`→`connect()` synthesis (§3) *is* netlist→circuit — a familiar EE mental model for the electrical layer. |

Bottom line: we are **standards-native at the modeling layer** (Modelica/MSL/acausal `connect()`), we
**realize SSP's system-structure concept in USD** rather than its XML, and we have a **latent FMI bridge**
(rumoca templates) for tool interop when needed. Nothing here fights the standards; USD is an additional,
richer structural carrier that can *export to* them.

---

## 5. Fit with our architecture & goal

- **Layering (`33-…md:26-29`) holds exactly.** USD = structure (which components exist, `epsBus`
  topology, parameters). Modelica = dynamics (the synthesized `Electrical.mo`, one DAE). Rust = reusable
  substrate (the synthesizer that reads USD→emits Modelica→`compile_str`; the co-sim master; the MSL
  session). The synthesizer is generic Rust, authored once, never per-vehicle.
- **Open registries, not enums.** Components declare their electrical contribution via USD attrs + MSL
  *class names* (open, string-addressed) — not a closed Rust electrical-component enum. New part types
  are new MSL classes / new component `.usda`, no core change.
- **Reusable-component composition (doc 36 §1).** The electrical layer is the first worked example: basic
  components → synthesized domain model → boundary-wired into the multi-domain vehicle → nested up into
  robots. Same rules at every level.
- **Digital-twin goal.** A rover whose electrical bus is a *real acausal circuit* built from reusable
  parts, hand-editable, co-simulated with thermal/GNC/comms and driven by the same clock/ephemeris as
  the physics — that is a genuine multi-domain twin, not a lumped hack. And because synthesis is
  text-canonical and MSL-standard, the model is inspectable, teachable, and exportable.

---

## 6. Watch-outs

- **Cold-compile / MSL-install latency.** First compile that pulls MSL Electrical eats MSL resolution
  (seconds warm, "minutes on cold `.cache/rumoca`" per `worker.rs:618`, attributed to parol
  Debug/`log::trace` overhead — see rumoca `docs/design-notes/perf-parol-trace-overhead.md`). Mitigate:
  pre-warm with `msl_indexer --warm`; a **pure-equation synthesized model (no MSL import) compiles fast
  on the first shot**. For a v1 electrical layer, consider emitting a *self-contained* `.mo` (inline the
  handful of R/C/Ground/Source equations) to skip MSL resolution entirely; graduate to MSL imports once
  the warm-cache path is reliable.
- **No test proves MSL-electrical steps to numbers end-to-end here.** `RC_Circuit.mo` is authored and
  the flatten/connect machinery is tested in rumoca, but usd's coverage test only asserts AST scan, not
  a numeric DAE solve. **Validate a real `connect()`-based electrical `.mo` stepping to correct numbers
  before building the synthesizer on top of it** (thin spike first).
- **Co-sim boundary lag is real.** The one-tick Jacobi lag across domains is fine for loose couplings
  but can bite tight ones — keep genuinely stiff couplings *inside* one DAE (that is the whole point of
  the §1 rule), and only cross a scalar wire where a tick of delay is physically harmless.
- **`instanceable` still dropped by flatten** (doc 36 §1.1) — many identical loads = N `references`, and
  the synthesizer emits N instance lines; fine, just not prototype-shared.

---

## 7. Recommended path

1. **Spike:** author one real `connect()`-based `Electrical.mo` (battery + bus + 2 resistive loads),
   `compile_str` it, confirm it steps to correct bus voltage / currents. Decide MSL-import vs
   self-contained based on cold-compile feel.
2. **Rule:** lock the two-level composition — acausal within domain (one DAE), causal across domains
   (scalar co-sim). Document the electrical layer as one `lunco:modelicaModel` prim + boundary ports.
3. **Synthesizer v1 (the new Rust):** read composed USD components + `lunco:epsBus` edges → emit
   `Electrical.mo` (string/`ast_mut`) → `compile_str` → `SimStepper`. Scaffold-and-own; explicit
   re-synthesis; hand-edits preserved (text-canonical).
4. **Manual editing UI:** reuse the existing Modelica diagram builder — it *already* does
   drag-MSL-parts→`.mo`; point it at the synthesized `Electrical.mo`.
5. **Boundary wiring:** scalar-wire the electrical model's V_bus/SoC/per-load ports to comms/thermal/GNC
   prims via the existing `lunco:simWires`/wire-prim → `SimConnection` path.
6. **Later tracks:** FMI export (rumoca `fmi2/fmi3` templates) for interop; live USD↔Modelica projection;
   `.ssp/.ssd` export; SysML v2 as the structural source-of-truth above USD.

The reusable comms component (doc 36 §1.2) is the first consumer: its `Power` layer contributes a load to the
synthesized rover electrical model, and its link dynamics stay a separate co-sim domain — exactly the
two-level rule in action.

---

## 8. Pluggable synthesizers: rhai policy + Rust backend + hooks

A synthesizer must **not** be one hardcoded Rust function. The netlist-mapping rules ("this component →
that MSL class", "insert a fuse here", "lowfi omits parasitic R") are *policy*, and the house directive is
**policy → rhai, primitives → Rust, dispatch → open registry**. Everything needed already ships — this
section is wiring, not new substrate.

### 8.1 What a synthesizer *is*

> A **synthesizer** is a named entry in a `SynthesizerRegistry` that reads the composed USD graph (+ params)
> and **emits an authored artifact** (Modelica text, USD scaffold, wire set…), which it then writes and
> compiles. Its *mapping policy* is rhai; its *primitives* (graph reads, model emit, compile) are Rust; its
> *lifecycle* is staged so hooks can veto/transform at each step.

This sits exactly between the two existing patterns the codebase already documents as complementary
(`lunco-scripting/src/scenario.rs:118-133` TODO(hooks)): **registered + name-dispatched like a
`lunco-hooks` policy fn**, and **lifecycle-staged like a `ScenarioRuntime`**. It is also the same
"param → authored artifact" shape as the **obstacle-field generator** (`lunco-obstacle-field`:
`ObstacleFieldSpec` → `build_height_grid` → `RegenerateField` command) — a synthesizer is that generator,
generalized to emit *models* and made rhai-pluggable.

### 8.2 Registry — model on `ApiQueryRegistry`, with the ControlKernel dual-backend trick

Every primitive below is a verified, existing pattern:

- **Registry shape** = `ApiQueryRegistry` (`lunco-api/src/queries.rs:86`): `register(name, provider)` /
  `get(name)`, a `Resource`, domain crates register at startup. Chosen because **rhai already reaches it
  for free** via `cmd(name,…)` / `query(name,…)` — no new bridge.
- **Built-in *or* scripted per entry** = the `ControlKernelRegistry` trick (`lunco-core/src/kernels.rs`):
  a name resolves to **either** a registered Rust `SynthProvider` **or** a `lunco-hooks` hook id (rhai).
  `DriveMix::new(hook_id)` + `apply_drive_mix` (registry-first, else scripted-kernel-via-`lunco_hooks`) is
  the exact "registered Rust default OR named rhai override, selected by name" precedent.

```rust
// substrate (new, tiny — mirrors ApiQueryRegistry)
trait SynthProvider { fn name(&self) -> &str; fn synthesize(&self, w:&mut World, spec:&Value) -> SynthResult; }
struct SynthesizerRegistry { providers: HashMap<String, SynthBackend> }   // Resource
enum SynthBackend { Builtin(Arc<dyn SynthProvider>), Scripted(HookId) }    // Rust default OR rhai hook
```

### 8.3 The rhai surface a synthesizer uses (all exist today)

A rhai synthesizer body needs to *read the graph*, *emit a model*, and *compile* — each is an existing verb
or `#[Command]` reachable via `cmd()`:

| Need | Existing primitive |
|---|---|
| enumerate components under the rover | `children(id)` → `bridge_core::children_of` (`bridge_core.rs:849`); `find`, `parent`, `name` |
| read a component's params / `lunco:` attrs | `param(id,"key")` → `ScriptParams`/`lunco:params` (`bridge_core.rs:577`); `get(id,"Comp.field")` |
| emit Modelica text into a doc | `cmd("SetDocumentSource", #{doc, source})` (`lunco-modelica/src/api/doc.rs:11`, `String` field filled from a rhai string) |
| structured emit (optional, robust) | expose `ast_mut` ops as verbs: `cmd("ApplyModelicaOp", #{op:"AddComponent", class, decl})` / `AddConnection` (`document/ops.rs:126`) — text-canonical patch |
| compile it | `cmd("CompileModel", #{doc, class})` (`ui/commands/compile.rs:38`, GUI-free by design) |
| read back status/source | `query("CompileStatus"/"GetDocumentSource"/"DescribeModel", …)` (`api_queries.rs:62`) |

So a minimal electrical synthesizer in rhai is: `children(rover)` → filter electrical → per component
`param()` its rating → build a `.mo` string (instance lines + `connect()` from the `epsBus` edges) →
`cmd("SetDocumentSource")` → `cmd("CompileModel")`. **No new Rust required for a v1** beyond registering the
name; graduate to `ApplyModelicaOp` verbs when you want structured/round-trippable emission instead of raw
string concat.

### 8.4 Hooks — the extension points (open string ids, `None` = built-in)

Reuse `lunco-hooks` verbatim: `lunco_hooks::invoke(id, &[HookValue]) -> Option<HookResult>` — `None` ⇒ the
synthesizer's built-in behavior runs; `Some` ⇒ the rhai hook overrode/vetoed. `HookValue` is the neutral
Int/Float/Bool/Str/Array/Map carrier (no JSON). rhai hooks are dropped in via
`register_rhai_hook(id, entry, src, deterministic)` (`lunco-hooks-rhai/src/lib.rs:67`), and a hook body can
itself `cmd()/get()` because hooks share the bridge engine. Lifecycle hook ids for the electrical
synthesizer (open strings, each a documented contract):

| Hook id | Fires | A hook can… |
|---|---|---|
| `synth.electrical.select` | after enumerating components, before mapping | filter/augment the component set (drop a load, inject a fuse/bus-bar) |
| `synth.electrical.map_component` | per component | override the chosen MSL class + params (the extensible netlist rule) — `None` = default map |
| `synth.electrical.emit` | after text built, before write | post-process the `.mo` text (annotations, param overrides, header) |
| `synth.validate` | after compile | inspect balance/DAE diagnostics → **veto** (fail-closed, like `rbac.authorize`) |
| `synth.on_recompile` | after a (re)synthesis lands | re-wire boundary ports / notify dependent domains |

This is the **MergePolicy / RBAC-authorize dispatch pattern** (`lunco-twin-journal:1396`,
`lunco-core/session.rs:839`) applied to synthesis: a registered Rust default that a rhai hook can restrict
or replace, deterministic-flagged, absent-hook ⇒ fall back. Because it rides `lunco-hooks`, a hook is
just as reachable from a different language backend later (Python, wasm) without touching the synthesizer.

### 8.5 Many synthesizers, one mechanism

The value is that this is **not comms- or electrical-specific**. The same registry + rhai + hooks yields a
family:

- `electrical` — USD components + `epsBus` → one acausal `Electrical.mo` (§3).
- `thermal` — thermal nodes + conductances → `ThermalNode` network (doc-34 gap, same shape).
- `harness` / `databus` — data topology → comms/OBC signal routing.
- `comms-link` — the doc-36 `CommsLink.mo` from antenna params.
- `wiring` — emit the *cross-domain* `SimConnection` boundary wires between the synthesized domain models
  (the causal half of §1), so even the co-sim wiring is a synthesizer, not hand-authored.
- `usd-scaffold` — a spec → a component-composed rover `.usda` (structure synthesis, the inverse direction).

Add a synthesizer = register a name + author a rhai body (+ optional Rust `SynthProvider` for a fast/default
path). No core change, no enum — the open-registry, `lunco-hooks`, `ApiQueryRegistry` substrates already
carry it. This is the **"less Rust / more dynamic"** directive realized: the *rules* for turning a
component graph into runnable multi-domain models live in rhai + USD, and Rust only owns the durable
primitives (graph reads, `compile_str`, the co-sim master, the registry).

### 8.6 Guardrails

- **Synthesis mutates a document → it must ride the journal + RBAC.** A synthesizer is a privileged
  doc-writer; route its `SetDocumentSource`/`ApplyModelicaOp` through the same `AuthorTag`/journal path
  tools use (`doc_ops::apply_one_op_as(..., AuthorTag::for_tool("synth"))`) so edits are undoable,
  attributable, and gated by the existing `rbac.authorize` hook. Never let a synthesizer clobber
  hand-edits silently — text-canonical patching + the scaffold-and-own stance (§3) preserve manual work.
- **Determinism.** Mark deterministic synthesizers/hooks so caching + replay hold; a synthesizer that reads
  wall-clock/ephemeris state must declare itself non-deterministic.
- **Loop safety.** A `synth.on_recompile` hook that re-triggers synthesis can loop — gate re-synthesis on an
  actual graph-hash change (mirror `RegenerateField`'s param-change guard), not on every recompile.
