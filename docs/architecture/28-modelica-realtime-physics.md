# 28 — Modelica Realtime Physics (declarative custom physics, networked)

> Status: Design · Audience: contributors planning declarative/networked Modelica physics (scopes Step 1)
>
> Goal: describe **most custom physics in Modelica** instead of hardcoding it
> in Rust — with proper solvers, running in realtime, safe under multiplayer,
> hot-changeable at runtime, and stepped as a first-class **ECS** citizen.
>
> This doc resolves the one hard tension in that goal (adaptive solvers vs
> deterministic multiplayer), classifies physics into three tiers by their
> networking role, and scopes **Step 1** — an ECS-native, server-authoritative
> Modelica stepper for a slow domain — as the lowest-risk entry point.

Builds directly on [`22-domain-cosim.md`](22-domain-cosim.md) (the FMI master
loop, `SimComponent`/`SimConnection`, USD-driven wires), [`14-simulation-layers.md`](14-simulation-layers.md)
(Participants-are-ECS-entities, `BackendCaps`), and the networking decisions in
[`../../crates/lunco-networking/DECISIONS.md`](../../crates/lunco-networking/DECISIONS.md)
(server-authoritative + client prediction, `SimTick`, wire-only feature gating D7).

## 1. The central tension

Two of the asks pull in opposite directions:

- **"Proper solvers."** Rumoca's solvers are **adaptive implicit** (BDF / diffsol).
  They pick step size from per-machine floating-point error estimates, so the
  same model on two peers takes different steps. The trajectory is *correct* but
  **not bit-reproducible across machines**.
- **"Multiplayer."** The client-prediction architecture (the one the steering-jitter
  work hardened — see the steering jitter and determinism designs) needs **fixed-step
  deterministic** integration: identical inputs ⇒ identical outputs on every peer,
  replayable for rollback. An adaptive solver in the prediction loop produces a
  *different* answer on the client than the server every tick ⇒ permanent
  reconciliation ⇒ the exact disease we just cured.

The resolution is **not** "Modelica everywhere, uniformly." It is to **classify
each physics model by its networking role**, and pick the solver + replication
strategy from that.

## 2. Three tiers

| Tier | Examples | Solver requirement | Networking | Modelica fit |
|------|----------|--------------------|------------|--------------|
| **A — fast, player-coupled, predicted** | chassis, contacts, joints, wheels, anything the player feels frame-to-frame | **must be fixed-step deterministic** (explicit / semi-implicit, bounded step = sim tick) | client-predicted + rollback | only once rumoca emits a **fixed-step deterministic** backend — **hard, upstream work** |
| **B — slow, server-authoritative, replicated** | thermal, power/battery, chemistry, ECLSS, aero, orbital | adaptive "proper" solver is **fine** | server computes; **outputs replicated as wires**, clients never predict them | **sweet spot** — adaptive solvers belong here; precedent already exists (gravity Shape A, [`22-domain-cosim.md`](22-domain-cosim.md)) |
| **C — local / cosmetic** | non-networked effects, visual-only | anything | none | free |

Most "custom physics" a user wants to author is **Tier B** — and Tier B is exactly
where adaptive Modelica solvers are *safe*, because clients **receive** state, they
do not **predict** it. No determinism contract, no rollback, no reconciliation.

**Anti-goal (load-bearing):** never put an adaptive-solver Modelica model directly
inside the client-prediction loop. That is Tier A, and Tier A needs a different
solver class.

### 2a. Implementation status of the tier contract (read this before trusting §2)

The tier contract above is a **design**. What is actually in the code, as of
2026-07-12 (finding `A4`):

| Piece | Status |
|---|---|
| `CosimTier` component (`A`/`B`/`C`) — `crates/lunco-cosim/src/connection.rs` | **implemented** |
| Declared in USD as `lunco:cosim:tier`, read at prim-read time (`lunco-usd-sim/src/cosim.rs`) | **implemented** |
| Gate: a non-Tier-A (or **undeclared**) model wiring a force/torque port on a client-predicted `Dynamic` body | **warns at wire-build time** (`rewire_usd_connections`); does **not** refuse the wire |
| `lunco:replication` → always-on `Replication` metadata (§5, §"declared in USD") | **not implemented** — no code reads it |
| Tier ↔ solver/caps validation at load ("rejected loudly on conflict") | **not implemented** |
| A fixed-step deterministic (Tier-A-grade) solver | **not available** — see below |

The live/interactive stepper no longer shares the batch runner's solver
configuration (it used to: adaptive-implicit BDF/diffsol, `atol = rtol = 1e-6`,
driven at 3 fixed sub-steps — an adaptive implicit solver inside the
client-predicted loop, i.e. precisely the §1 anti-goal). The live path now has its
own configuration, in `worker::live_stepper_options`:

- **explicit family** (`SimSolverMode::RkLike`) — no Newton/LU iteration whose
  *count* varies with the machine's rounding;
- a **fixed micro-step ladder**: every macro step is an integer number of
  `LIVE_MICRO_DT = SECS_PER_TICK / 3` micro-steps (`micro_steps_for(dt)`), so the
  model's stop-time lattice is a pure function of the fixed-step clock and the
  requested `dt` — identical on every peer;
- a fixed tolerance, **not** the model's `experiment(Tolerance=…)` annotation (an
  offline-accuracy knob must not reach into the realtime loop).

**This is not yet Tier-A-grade determinism, and the doc will not pretend it is.**
rumoca's `RkLike` backend is an *embedded* RK45: its internal sub-step size is
still error-adapted (`adapt_step(h, error_norm)`), so a micro-step may split
differently on two machines. rumoca exposes no fixed-tableau, error-control-free
stepper today. Driving it at fixed micro-steps bounds the divergence to *within*
one micro-step and pins the macro stop-times, which is as far as the client layer
can go alone.

> **TODO(A4)** — to close this properly:
> 1. *Upstream (rumoca):* a fixed-step tableau with no error control — the
>    "Realtime profile" of §7. This is the load-bearing missing piece; until it
>    lands, no Modelica model is genuinely Tier A.
> 2. *Enforcement:* promote the `rewire_usd_connections` warn to a **refusal**
>    (drop the wire, surface a diagnostic) once scenes actually declare
>    `lunco:cosim:tier`. Enforcement point:
>    `crates/lunco-usd-sim/src/cosim.rs::rewire_usd_connections`, gate function
>    `lunco_cosim::CosimTier::may_drive_predicted_physics`.
> 3. *Replication:* wire `lunco:replication` (§5) to the tier, or delete it from
>    this doc in favour of `lunco:cosim:tier` — today it is authored nowhere and
>    read nowhere.

## 3. Realtime budget

Adaptive implicit solvers can blow a frame budget on stiff systems — we have
already hit `BDF step too small` and worker OOM on `RoverThermalSystem` / `AbdulezerPair` (see solver regressions and the responsive UI mandate). Realtime therefore needs a **bounded-compute contract**, independent of tier:

1. **Off the render thread.** Heavy steppers run on the worker / server tick
   (already true — rumoca runs on a worker thread per [`22-domain-cosim.md`](22-domain-cosim.md)),
   never blocking the main loop. A runaway stepper fails its *run*, it does not
   stall the app.
2. **Sub-rate.** Tier-B domains change slowly — step thermal at 5–10 Hz, not 60.
   Decouple the model's clock from the 60 Hz `SimTick` (the multi-clock hook in
   [`14-simulation-layers.md`](14-simulation-layers.md) §multi-clock).
3. **Step budget.** Cap solver substeps per communication step; on exceed,
   degrade fidelity rather than spin (the `FidelityPolicy` hook), and surface it
   — never silently freeze.

Tier B tolerates all three naturally. Tier A cannot sub-rate and cannot exceed
its step budget — another reason Tier A is the hard tier.

## 4. ECS-native cosim

The substrate is **already ECS**, and [`14-simulation-layers.md`](14-simulation-layers.md)
already states the principle "**Participants are ECS entities**." Make the Modelica
stepper a full ECS citizen by mapping every part of a model onto ECS:

| Model concept | ECS representation |
|---|---|
| model instance | an **entity** (tagged `ModelicaModel` + `SimComponent`, today) |
| inputs / outputs | **port components** (`SimComponent.inputs/outputs`, surfaced as `DigitalPort`/`PhysicalPort` where they cross to hardware) |
| state vector | a **component** on that entity (the compiled stepper's state lives in-world, snapshot-able) |
| one integration step | a **`FixedUpdate` system** reading inputs, stepping, writing outputs |
| coupling between models | an **ECS wire** (`SimConnection`) — identical to the gravity Shape A wire |
| replication | the **existing networking wire layer** — a Tier-B output wire becomes networkable for free |

The pieces in **bold** that don't fully exist yet (state-as-component,
snapshot/restore via `Participant::checkpoint`) are the additive work. The
payoff: a Modelica physics model is wired, stepped, paused, time-warped,
checkpointed, and **replicated** by the same machinery as everything else — and
multiplayer-safe in Tier B without one line of new netcode (the wire layer
already replicates).

## 5. Tiers select the replication mechanism

The tier is not just a solver choice — it **decides how a model is duplicated
across peers**. There is one axis: *what do we duplicate — the computation, the
result, or nothing?* The tier answers it, and that answer picks one of the
networking sync mechanisms (M1–M7 in replicated state sync architecture).

| Tier | What is duplicated | What crosses the wire | Sync mechanism |
|------|--------------------|------------------------|----------------|
| **A — predicted** | the **computation** — the deterministic stepper runs on **both** peers | **inputs** (op-log / commands) + periodic **authoritative state correction** for reconciliation | client-prediction + state-correction (the rover path today) |
| **B — server-authoritative** | only the **result** — stepper runs on the **server alone**, client does **not** integrate | **output state** (the model's ports / state component) | state replication (the gravity Shape A wire) |
| **C — local** | **nothing** | nothing | none |

So the duplication question — *"run this model on the client too, or just stream
its state?"* — is answered by the tier, not decided per-model ad hoc. This is the
key payoff of the classification: it turns "what do we replicate?" from a
case-by-case judgement into a lookup.

### The tier is **declared in USD**, never inferred

The tier must not be guessed from a heuristic (component name, "does it have a
RigidBody", etc.) — it is **authored on the prim**, the same way mass, friction,
and the cosim model already are:

```usda
def Xform "RoverBattery" (prepend apiSchemas = ["LuncoReplicationAPI"])
{
    token  lunco:replication = "authoritative"   # local | authoritative | predicted  (Tier C | B | A)
    string lunco:modelicaModel = "models/RoverBattery.mo"
    string lunco:simWires      = "load_w:..."
}
```

`lunco:replication` ∈ `{local, authoritative, predicted}` = Tier C / B / A. The USD
translator (`lunco-usd-sim`) reads it at spawn and sets the **always-on
`Replication` metadata** for that entity — the registry the networking layer
consults (PH2 `declare_replication::<C>(Replication)`). This is the same move that
already removed field-name heuristics from the id/authz codec: schema-driven
`WireLocal` / `AuthzTarget` reflect markers instead of guessing by field name
(see typed command and serialization codec). The wire layer reads a **declared** tier; it never
infers one.

Practicalities:

- **Defaults by prim type / applied schema** so authors don't repeat themselves: a
  `LuncoReplicationAPI` applied schema (or a per-type default) supplies the tier;
  it inherits down namespace like any USD attribute. **Unspecified ⇒ `local`** (the
  safe default — a model is never silently replicated).
- **Declared intent is still validated, not trusted blindly.** A prim tagged
  `predicted` (Tier A) whose model/solver isn't fixed-step deterministic is a
  *conflict* — rejected loudly at load (ties to the Realtime-profile compiler gate,
  §7), never silently downgraded. USD removes the heuristic; the loader still
  type-checks tier ↔ solver/caps consistency.

- **Tier B** — server runs the authoritative stepper; output ports replicate to
  clients as wires over the existing networking channel (D7: gated behind the
  `networking` feature; in solo the wire is local and there is no replication —
  the architecture degrades to single-player *by construction*, matching
  prediction and reconciliation strategy's "solo reconcile is a structural no-op").
  Clients render received state; they never integrate it. No determinism needed.
- **Tier A** — both peers run the **same** stepper, so it requires (1) a
  fixed-step **deterministic** solver and (2) a determinism contract (same
  fold/step order on every peer, integer `SimTick` clock, no `Date::now`/`Math::random`
  — mirrors the replicated state sync architecture identity rules). Until both exist,
  Tier-A physics stays in deterministic Rust (avian + the mobility force laws),
  with Modelica used only as an **offline oracle** (§8, Step 2).

## 6. Robotics-ready: custom solvers per model

Robots break the "one global solver" assumption: a manipulator's articulated-body
dynamics, a contact-rich gait, and a real-time control loop each want a *different*
integrator (fixed-step semi-implicit for stable contact, RK4 for smooth dynamics,
or an external real-time loop for a controller). The architecture must let **each
model bring its own solver** — and it already can, because the cosim master loop
only ever calls `participant.step(dt)` ([`14-simulation-layers.md`](14-simulation-layers.md)
`Participant` trait). The solver lives *inside* the participant; the master loop
is solver-agnostic.

Making this first-class:

- **Solver is a per-participant property**, selectable at authoring time (a USD
  attribute / model annotation, e.g. `lunco:solver = "rk4-fixed"`), not a global
  setting. The `BackendCaps.native_solver` flag already distinguishes models that
  carry their own integrator from those needing an external one (FMU-ME style).
- **Robotics fast dynamics + control is Tier A** — deterministic, fixed-step,
  often at a control rate distinct from render (the multi-clock hook). So robotics
  is the **forcing function** for Step 3: the fixed-step deterministic solver path
  Tier A needs is exactly what a robot's controller/dynamics loop needs. A robot
  is not a special case bolted on — it is the canonical Tier-A custom-solver
  citizen.
- **External / HIL solvers** (a ROS 2 node, a Copper rate-group, real hardware in
  the loop) plug in as a **Backend** whose `step()` advances an external loop and
  whose ports bridge ROS topics ↔ `SimConnection` wires. This is the
  ROS2/Copper-as-bridge path already in replicated state sync architecture — a robot
  controller running its own solver is just another participant on the wire.
- **Custom solvers stay inside the tier contract**: a Tier-A custom solver must be
  fixed-step + deterministic (or it isn't predictable); a Tier-B custom solver may
  be anything (it only streams state). The tier still selects the replication
  mechanism regardless of which solver the participant carries.

## 7. Hot-changeable behaviour (incl. vehicle physics at runtime)

Two distinct flavours, different cost:

- **Parameter change** (coefficients, setpoints): cheap. Compile-once + runtime
  parameters (the roadmap item in parallel experiment execution §2b) → feed as
  input wires / `ControlStream` live inputs ([`22-domain-cosim.md`](22-domain-cosim.md)
  control-vs-data plane). No recompile.
- **Structural change** (swap equations / whole model): needs recompile, then
  **hot-swap the compiled stepper** (`BackendCaps.supports_live_swap` reserves this).

How runtime control plays out **depends on the tier** — and vehicle physics is
Tier A, the hard one:

- **Tier B (server-authoritative):** either flavour is loose — mutate on the
  server, replication carries the new behaviour to clients. No coordination.
- **Tier A (vehicle / predicted):** a runtime change must be applied **identically
  on every peer at the same tick**, or prediction desyncs. So it rides the
  **deterministic command/op-log channel** (not a local ad-hoc mutation) and lands
  at a tick boundary — then every peer's stepper is reconfigured in lockstep and
  reconciliation stays quiet.

**Vehicle physics is already nearly there at the parameter level.** The mobility
force laws were just refactored so every knob is explicit and USD-authored —
`DEFAULT_DRIVE_FORCE_PER_NORMAL`, per-wheel `friction_mu`, `contact_grip_stiffness`,
suspension `spring_k`/`damping_c`, motor `peak_torque`. Exposing those as runtime
parameters routed through the deterministic command channel gives **live tuning of
vehicle handling, multiplayer-safe**, with no Modelica and no Step 3 — the integration
stays fixed-step deterministic Rust; only the coefficients change, in lockstep.
That is the practical "control vehicle physics at runtime" path available now.

**Structural** vehicle change (swap the whole friction/suspension *model*, e.g. to
a Modelica-described one) is the Tier-A hot-swap: only once Step 3's fixed-step
deterministic Modelica lands, and only at a quiesced tick boundary applied across
all peers — never mid-rollback.

## 7. The realtime Modelica profile (the Tier-A path)

The way to make Tier-A physics describable in Modelica is **not** to make rumoca's
general adaptive solver deterministic. It is to define a **restricted profile** —
a special fixed-step deterministic solver **plus limitations on the model**, with
the model still authored in plain Modelica code. The compiler is the gate: a model
either type-checks into the *Realtime profile* (and is then predictable +
multiplayer-safe by construction) or it is rejected with a clear reason. This is
how every realtime/HIL Modelica toolchain works (inline integration, fixed-step
code-gen subsets).

**The special solver:** fixed-step, fixed work per step — semi-implicit (symplectic)
Euler for the common non-stiff case (the same class as the gold-standard
`wheel_spin.rs`), or a fixed-step **linearly-implicit** method (Rosenbrock-1 /
implicit Euler with a *fixed* iteration count) for mild stiffness. Determinism comes
from: fixed step count, fixed iteration count, fixed evaluation order, integer
`SimTick` clock, no wall-clock / RNG, identical IEEE float ops on every peer.

**The property limitations** (compiler-enforced — the profile's "type system"):

- **Fixed structure** — no variable-structure systems, constant state count.
- **Fixed-step-stable dynamics** — reject systems whose stiffness needs adaptive
  steps to stay stable at the chosen `dt` (or require the linearly-implicit solver).
- **Bounded state** — guards against runaway (the responsive UI mandate
  invariant); a model that can diverge in finite ticks is rejected.
- **Tick-quantized events** — zero-crossings/events resolve **at tick boundaries**,
  not via intra-step root-finding (root-finding makes step timing data-dependent →
  non-deterministic across peers).
- **Deterministic evaluation order** — fixed fold order, no wall-clock/random.

This is the same profile **robots** want (§6): a controller / articulated-body
loop is exactly a fixed-step, bounded, deterministic Tier-A model. Robots and
vehicles are the two canonical Realtime-profile citizens.

## 8. Staged roadmap

1. **Step 1 — Tier B, ECS-native, server-authoritative (this doc, §9).** One slow
   domain modelled in Modelica, stepped as an ECS system, output replicated as a
   wire. Proves *all* the asks (declarative physics + realtime + multiplayer +
   hot-param + ECS-native) inside the safe tier, reusing cosim + networking that
   already exist. Lowest risk, highest signal.
2. **Step 2 — the oracle.** A Modelica quarter-car / wheel-friction reference run
   headless via the experiment path, compared against the Rust `suspension_force_mag`
   / `contact_friction` / `drive_force_mag` force laws (now extracted as pure,
   testable functions in `lunco-mobility`). Modelica as **ground truth, out of the
   loop** — would have caught the explicit-Euler limit-cycles immediately. Validates
   Tier-A Rust physics without committing to runtime Modelica.
3. **Step 3 — Tier A in the loop (the hard ask): the Realtime profile (§7).** Build
   the special fixed-step deterministic solver + the compiler-enforced property
   limitations, so a vehicle/robot model authored in (restricted) Modelica can run
   *inside* the prediction loop. Highest risk; do last, once 1–2 have shown value.
   Tier-A *parameter* tuning (§7) is available well before this — Step 3 is only
   needed to replace the force-law *structure* with Modelica.

## 9. Step 1 scope — ECS-native Tier-B Modelica stepper

**Demonstrator:** rover **battery State-of-Charge** (alternative: a thermal node).
Chosen because it (a) is genuinely slow/server-authoritative, (b) couples
naturally to the rover already being driven (electrical load ≈ motor torque · ω
from `lunco-hardware`), (c) is player-visible (a battery gauge), and (d) is a
clean scalar ODE that cannot blow the step budget.

```modelica
model RoverBattery
  input  Real load_w   = 0;     // electrical load (W), wired from motor draw
  parameter Real capacity_wh = 1000;
  parameter Real v_nominal   = 28;
  Real soc(start = 1.0);        // 0..1
  output Real voltage;          // observable → must be `output` (rumoca convention)
equation
  der(soc) = -load_w / (capacity_wh * 3600);
  voltage  = v_nominal * (0.9 + 0.1 * soc);
end RoverBattery;
```

**Deliverables (build on what exists — no Twin/BackendRegistry refactor required):**

1. **Authoring** — declare the model + wires in USD, reusing the existing
   `lunco-usd-sim` cosim attributes (`lunco:modelicaModel`, `lunco:simWires`,
   cross-entity `wireFrom/wireTo`). The battery entity wires `load_w` ← rover
   motor power and exposes `soc`/`voltage` outputs. **Zero new Rust to author.**
2. **ECS stepper** — confirm the model steps via the existing `FixedUpdate` cosim
   pipeline (`sync_modelica_outputs` → `propagate_connections` → `sync_inputs_to_modelica`
   → worker step), gated on the sim running (`Time<Virtual>.relative_speed > 0`) and sub-rated to ~10 Hz
   (every Nth `SimTick`), running on the worker thread.
3. **State-as-component** — store the stepper's `soc` on the entity as a small
   replicated component (`BatteryState { soc, voltage }`), the first concrete
   instance of "state vector = component" (§4). Snapshot/restore wired to
   `Participant::checkpoint`/`restore` for reset + late-join.
4. **Replication** — register `BatteryState` (or just its output port) on the
   existing networking wire/snapshot channel behind the `networking` feature (D7).
   Server steps; clients receive. **Solo:** local, no replication, no behaviour
   change — verifies the "degrades to single-player by construction" property.
5. **Hot-param** — `capacity_wh` / `v_nominal` settable live via `SetModelInput`
   / `ControlStream` (no recompile), proving runtime behaviour change.
6. **Readout** — surface `soc`/`voltage` in telemetry (existing trace + plots);
   no new panel infra.

**Acceptance:**
- Driving the rover drains the battery; gauge falls in realtime, identical native
  and (once feature-on) replicated to a client with no client-side integration.
- Pause freezes `soc`; resume continues; reset restores `soc = 1.0` via checkpoint.
- Changing `capacity_wh` live changes the drain rate mid-run.
- Worker stepping never stalls the main loop (kill the worker → run fails, app
  survives — the responsive UI mandate invariant).

**Explicitly out of scope for Step 1:** any Tier-A model, rumoca fixed-step
codegen, the offline oracle (Step 2), structural hot-swap, the full Twin /
BackendRegistry formalisation.

## 10. Decision log

1. **Classify physics by networking role, not uniformly "Modelica everywhere."**
   Tier A (predicted) ≠ Tier B (replicated) ≠ Tier C (local). The tier also
   **selects the replication mechanism** (duplicate computation / duplicate state /
   nothing — §5), turning "what do we replicate?" into a lookup.
2. **Adaptive solvers are for Tier B only.** They are non-deterministic across
   peers and must never enter the client-prediction loop (Tier A).
2a. **The tier is declared in USD, never inferred.** *Implemented* as
   `lunco:cosim:tier ∈ {A, B, C}` → the `CosimTier` component, read at prim-read
   time and gated at wire-build time (§2a). *Designed, not implemented:*
   `lunco:replication` → the always-on `Replication` metadata at spawn, and the
   load-time tier ↔ solver/caps validation ("rejected on conflict"). Undeclared is
   **not** Tier A: it may not drive predicted physics.
3. **Tier A Modelica = a Realtime profile (§7): a special fixed-step deterministic
   solver + compiler-enforced model limitations**, authored in plain Modelica. Not
   "make the adaptive solver deterministic" — constrain the models instead. Robots
   and vehicles are the canonical citizens. Until it exists, Tier-A physics stays in
   deterministic Rust;
   Modelica serves Tier A only as an offline oracle.
4. **The Modelica stepper is an ECS citizen**: instance = entity, ports =
   components, state = component, step = system, coupling = `SimConnection` wire,
   replication = the existing wire layer. No bespoke runtime.
5. **Tier-B multiplayer is free**: server steps, output wire replicates, solo
   degrades to local with no reconciliation by construction.
6. **Realtime safety = bounded compute**: off-thread stepping, sub-rate,
   step-budget-with-degrade. Never silently stall; a runaway model fails its run.
7. **Solver is a per-participant property, not global** (the `step(dt)` contract +
   `native_solver` cap). This is what makes the system **robotics-ready**: each
   robot/model brings its own solver; external/HIL solvers (ROS 2 / Copper) plug in
   as Backends bridging topics ↔ wires.
8. **Hot-param is cheap (runtime params), hot-structure is a stepper hot-swap**
   (Tier B any time; Tier A only at quiesced tick boundaries).
9. **Vehicle (Tier-A) physics is runtime-controllable now at the parameter level**:
   the extracted USD-authored knobs, routed through the **deterministic command
   channel** and applied at a tick boundary, give multiplayer-safe live handling
   tuning without Modelica. Structural change waits for the Realtime profile.
10. **Step 1 reuses existing cosim + networking**, adds only state-as-component +
   one replicated output — no Twin/BackendRegistry refactor as a prerequisite.

## See also

- [`22-domain-cosim.md`](22-domain-cosim.md) — the master loop, `SimConnection`, USD wires
- [`14-simulation-layers.md`](14-simulation-layers.md) — Participants-are-entities, `BackendCaps`
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica/rumoca specifics + `output` convention
- [`../../crates/lunco-networking/DECISIONS.md`](../../crates/lunco-networking/DECISIONS.md) — D1–D7, SimTick, wire-only gating
- `lunco-mobility/src/lib.rs` — the Rust Tier-A force laws Step 2 will validate
