# 28 — Modelica Realtime Physics (declarative custom physics, networked)

> Status: Design + enforced runtime contract · Audience: contributors planning declarative/networked Modelica physics (scopes Step 1)
>
> Goal: describe **most custom physics in Modelica** instead of hardcoding it
> in Rust — with proper solvers, running in realtime, safe under multiplayer,
> hot-changeable at runtime, and stepped as a first-class **ECS** citizen.
>
> This doc resolves the one hard tension in that goal (adaptive solvers vs
> deterministic multiplayer), draws the one line that matters — **may this program
> drive a force on a body the client predicts?** — and scopes **Step 1** — an
> ECS-native, server-authoritative Modelica stepper for a slow domain — as the
> lowest-risk entry point.

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

The resolution is **not** "Modelica everywhere, uniformly." It is to ask **one
question of every program** — *may this thing drive a force on a body the client
PREDICTS?* — and pick the solver + replication strategy from the answer.

## 2. The one line: the realtime-safe promise

A program that drives a `force_*` / `torque_*` port on a client-predicted body runs on
**both** peers, every tick. If it is late, or if it answers differently on two machines,
the client diverges from the server every frame it is late. So driving physics is a
**promise the author makes**, not a property the engine can read off the source:

```usda
uniform bool lunco:program:realtimeSafe = true
```

| | Realtime-safe (`= true`) | Everything else (the default) |
|---|---|---|
| Examples | chassis, contacts, joints, wheels, a lander's flight-control law — anything the player feels frame-to-frame | thermal, power/battery, chemistry, ECLSS, aero, orbital, a supervisory script, a cosmetic effect |
| Solver requirement | **must be fixed-step deterministic** (explicit / semi-implicit, bounded step = sim tick) | an adaptive "proper" solver is **fine** |
| Networking | client-predicted + rollback: both peers run the same stepper | server computes; **outputs replicated as wires**, clients never predict them (or purely local, and nothing crosses the wire) |
| Modelica fit | the restricted **fixed-rk4** profile is available for continuous, event-free, external-table-free models; broader semantics remain explicitly unsupported | **sweet spot** — adaptive solvers belong here; precedent already exists (gravity Shape A, [`22-domain-cosim.md`](22-domain-cosim.md)) |

Most "custom physics" a user wants to author never touches a predicted body — and that
is exactly where adaptive Modelica solvers are *safe*, because clients **receive**
state, they do not **predict** it. No determinism contract, no rollback, no
reconciliation. Such a program leaves `realtimeSafe` alone; it is not a quality rating,
and there is nothing to disclaim.

**Anti-goal (load-bearing):** never put an adaptive-solver Modelica model directly
inside the client-prediction loop. That is what the promise gates, and a program that
makes it needs a different solver class.

### 2a. Implementation status of the promise (read this before trusting §2)

What is actually in the code:

| Piece | Status |
|---|---|
| `uniform bool lunco:program:realtimeSafe`, read at prim-read time (`lunco-usd-sim/src/cosim.rs`) → the `RealtimeSafe` component (`crates/lunco-cosim/src/connection.rs`) | **implemented** |
| Gate: a program **without** `RealtimeSafe` wiring a force/torque port on a client-predicted `Dynamic` body | **rejects the force loop at wire-build time** (`rewire_usd_connections`) and raises a terminal runtime fault |
| Causal feedback in the explicit `SimConnection` fabric | **valid dynamic feedback**; the master performs one read/write transaction and does not pretend to solve an algebraic equation |
| Acausal connection/island | **not accepted by the causal fabric**; it requires a typed backend island and partition before stepping |
| `lunco:replication` → always-on `Replication` metadata (§5, §"declared in USD") | **not implemented** — no code reads it |
| Promise ↔ solver/caps validation | **implemented** (2026-07-27) — `lunco_experiments::solver` resolves by capability and refuses an incapable pairing at stepper construction |
| A fixed-step deterministic Modelica solver good enough to honour the promise | **implemented as `fixed-rk4`** for the restricted continuous profile; construction rejects events, discrete assignments, clocks, and external tables |
| Effect of the promise on live solver choice | **enforced** — `solver::resolve` admits prediction only for a backend declaring both `fixed_step` and `deterministic`; `fixed-rk4` is selected for qualified predicted Modelica programs while `rk45` remains authoritative-live only |

### Solver selection is resolved, not hardcoded (2026-07-27)

The live path used to name its own solver family, and that was a second,
independent selection site: `worker::live_stepper_options` hardcoded the explicit
family for **every** live model while the batch path resolved one properly. The
two silently disagreed.

MEASURED CONSEQUENCE: a rover's generated electrical island (battery + solar
panel) carries algebraic unknowns. Handed the explicit stepper it failed every
step with `rk45 backend only supports a narrow explicit ODE subset: algebraic
refresh row 2 cannot be solved for …Battery.p.i`, published no ports, and
reported nothing a driver would see. Every solar rover in the repo was dead,
including the shipped `scenes/luncosim/solar_rover_demo.usda` — which is the class
of model §2 above calls the adaptive solver's *sweet spot*.

Both paths now call `lunco_experiments::solver::resolve`, from two facts — and
**both are facts about WHERE the model runs**, never claims about the model:

- **`live`** — is it stepped inside the fixed-step frame loop, as opposed to an
  offline batch solve that owns its own time;
- **`predicted`** — does it drive a client-predicted body under the declared
  `RealtimeSafe` contract, carried to the worker on `ModelicaCommand::Compile`
  because the worker thread has no ECS.

There is deliberately **no** third fact for whether the model is *solvable*, and
the first attempt at one is worth recording because it was plausible and wrong:
`implicit = (dae.variables.algebraics non-empty)`, read off the compiled DAE. A
healthy battery island has algebraic variables too, so that predicate routed live
islands onto the adaptive backend — which owns its own step sequence and stalled
the worker so badly that one of three models finished compiling in 30 s of sim
time, with no error. Solvability is the backend's own lowering to answer and the
authored domain rules to constrain. A thin substrate does not get an opinion.

### The registry is the vocabulary

`SolverChoice` was a closed enum; it is a registry now, and that changes the
public surface in two ways worth stating plainly:

- **The old spellings are gone, not aliased.** Ids normalise case, `-`, `_` and
  spaces, so a stored `"rk_like"` normalises to `"rklike"` and matches no
  registered backend (the registered id is `"rk45"`). An API caller or saved
  `RunBounds` carrying a pre-registry name gets an `Unknown` error naming every
  valid id. There is no compatibility shim: one form, discoverable.
- **`ListSolvers` is how you discover them** (`lunco-modelica/src/api_queries.rs`)
  — id, label, rank and every capability flag, straight from the registry. It
  exists so that "what may I pass?" has an answer that cannot drift from what
  `resolve` accepts.

An unknown or incapable authored solver **fails the run**. It is not downgraded to
one that happens to work — `parse_solver_arg` used to warn and drop, which meant a
typo produced numbers under a different integrator than the caller believed, with
only a log line to say so. Refusal is reported to the console, not just the log,
because a run that never started otherwise looks like one still compiling.

What remains genuinely live-specific is stepping POLICY, not solver choice:

- a **fixed micro-step ladder**: every macro step is an integer number of
  `LIVE_MICRO_DT = SECS_PER_TICK / 3` micro-steps (`micro_steps_for(dt)`), so the
  model's stop-time lattice is a pure function of the fixed-step clock and the
  requested `dt` — identical on every peer;
- a fixed tolerance, **not** the model's `experiment(Tolerance=…)` annotation (an
  offline-accuracy knob must not reach into the realtime loop).

The current `rk45` backend is an *embedded* RK45: its internal sub-step size is
error-adapted, so it is not a fixed-step deterministic implementation. It is
registered for authoritative live co-simulation only. The `fixed-rk4` backend
owns an explicit four-stage integration loop over Rumoca's lowered derivative
runtime, advances on one exact configured lattice, and is registered with the
required prediction capabilities. It refuses unsupported model constructs at
construction; there is no tolerance path that knowingly permits peer divergence.
Adding events, discrete state, or external tables requires a new backend profile
with its own deterministic semantics, not a fallback to adaptive stepping.

The USD wiring gate independently refuses a force/torque edge from a program
without `RealtimeSafe` when the target is a client-predicted dynamic body. It
raises `cosim-predicted-force-contract` and holds physics, so a rejected edge
cannot leave a predicted body running with an unvalidated force path.

## 3. Realtime budget

Adaptive implicit solvers can blow a frame budget on stiff systems — we have
already hit `BDF step too small` and worker OOM on `RoverThermalSystem` / `AbdulezerPair` (see solver regressions and the responsive UI mandate). Realtime therefore needs a **bounded-compute contract**, independent of any promise:

1. **Off the render thread.** Heavy steppers run on the worker / server tick
   (already true — rumoca runs on a worker thread per [`22-domain-cosim.md`](22-domain-cosim.md)),
   never blocking the UI/update loop. For a causal participant, the worker is
   asynchronous in wall-clock execution while the shared simulation remains at
   one deterministic communication point until the result arrives. An
   independent participant may finish without holding the world; its last
   validated output remains zero-order-held. The app remains responsive and a
   failed worker step becomes a durable model fault rather than being accepted
   as a successful result.
2. **Sub-rate.** Slow domains change slowly — step thermal at 5–10 Hz, not 60.
   The live master now uses the authored
   `lunco:program:communicationPeriod` as the co-simulation boundary (0.1 s by
   default). It samples inputs and holds the last validated output between
   points, while the 60 Hz `SimTick`, Avian, and Rhai continue. A model that
   needs a fixed-tick exchange authors `1/60` explicitly; this is a causal
   communication policy, not a wall-clock throttle.
3. **Step budget.** Cap solver substeps per communication step; on exceed,
   report the participant failure and hold the shared simulation rather than
   silently degrading a state that other participants consume. An explicitly
   independent domain may choose a separate communication policy, but that is
   a declared co-simulation boundary, not an implicit Modelica fallback.

A program that makes no realtime promise tolerates all three naturally. One that DOES
cannot sub-rate and cannot exceed its step budget — which is exactly why the promise is
the hard thing to keep.

## 4. ECS-native cosim

The substrate is **already ECS**, and [`14-simulation-layers.md`](14-simulation-layers.md)
already states the principle "**Participants are ECS entities**." Make the Modelica
stepper a full ECS citizen by mapping every part of a model onto ECS:

| Model concept | ECS representation |
|---|---|
| model instance | an **entity** (tagged `ModelicaModel` + `SimComponent`, today) |
| inputs / outputs | **port components** (`SimComponent.inputs/outputs`, surfaced as `Port` where they cross to hardware) |
| state vector | a **component** on that entity (the compiled stepper's state lives in-world, snapshot-able) |
| one integration step | a **`FixedUpdate` system** reading inputs, stepping, writing outputs |
| coupling between models | an **ECS connection** (`SimConnection`) — identical to the gravity Shape A coupling |
| replication | the **existing networking wire layer** — a server-authoritative output wire becomes networkable for free |

The pieces in **bold** that don't fully exist yet (state-as-component,
snapshot/restore via `Participant::checkpoint`) are the additive work. The
payoff: a Modelica physics model is wired, stepped, paused, time-warped,
checkpointed, and **replicated** by the same machinery as everything else — and
multiplayer-safe, for every program that does not drive predicted physics, without one
line of new netcode (the wire layer already replicates).

## 5. What is duplicated across peers

There is one axis: *what do we duplicate — the computation, the result, or nothing?*
The answer picks one of the networking sync mechanisms (M1–M7 in replicated state sync
architecture).

| Role | What is duplicated | What crosses the wire | Sync mechanism |
|------|--------------------|------------------------|----------------|
| **predicted** (a realtime-safe program driving a force on a predicted body) | the **computation** — the deterministic stepper runs on **both** peers | **inputs** (op-log / commands) + periodic **authoritative state correction** for reconciliation | client-prediction + state-correction (the rover path today) |
| **server-authoritative** | only the **result** — stepper runs on the **server alone**, client does **not** integrate | **output state** (the model's ports / state component) | state replication (the gravity Shape A wire) |
| **local** | **nothing** | nothing | none |

So the duplication question — *"run this model on the client too, or just stream
its state?"* — is answered by what the program is wired to and what it promised, not
decided per-model ad hoc.

### The replication role is **declared in USD**, never inferred

It must not be guessed from a heuristic (component name, "does it have a RigidBody",
etc.) — it is **authored on the prim**, the same way mass, friction, and the program
itself already are:

```usda
def Xform "RoverBattery" (prepend apiSchemas = ["LuncoReplicationAPI"])
{
    token lunco:replication = "authoritative"        # local | authoritative | predicted

    def Scope "Battery" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @models/RoverBattery.mo@
        float inputs:load_w.connect = </Rover/Motor.outputs:power>
    }
}
```

`lunco:replication` ∈ `{local, authoritative, predicted}`. The USD
translator (`lunco-usd-sim`) reads it at spawn and sets the **always-on
`Replication` metadata** for that entity — the registry the networking layer
consults (PH2 `declare_replication::<C>(Replication)`). This is the same move that
already removed field-name heuristics from the id/authz codec: schema-driven
`WireLocal` / `AuthzTarget` reflect markers instead of guessing by field name
(see typed command and serialization codec). The wire layer reads a **declared** role; it
never infers one.

Practicalities:

- **Defaults by prim type / applied schema** so authors don't repeat themselves: a
  `LuncoReplicationAPI` applied schema (or a per-type default) supplies the role;
  it inherits down namespace like any USD attribute. **Unspecified ⇒ `local`** (the
  safe default — a model is never silently replicated).
- **Declared intent is still validated, not trusted blindly.** A prim tagged
  `predicted` whose program is not `realtimeSafe`, or whose solver isn't fixed-step
  deterministic, is a *conflict* — rejected loudly at load (ties to the
  Realtime-profile compiler gate, §7), never silently downgraded. USD removes the
  heuristic; the loader still type-checks role ↔ promise ↔ solver/caps consistency.

- **Server-authoritative** — the server runs the stepper; output ports replicate to
  clients as wires over the existing networking channel (D7: gated behind the
  `networking` feature; in solo the wire is local and there is no replication —
  the architecture degrades to single-player *by construction*, matching
  prediction and reconciliation strategy's "solo reconcile is a structural no-op").
  Clients render received state; they never integrate it. No determinism needed.
- **Predicted** — both peers run the **same** stepper, so it requires (1) a
  fixed-step **deterministic** solver and (2) a determinism contract (same
  fold/step order on every peer, integer `SimTick` clock, no `Date::now`/`Math::random`
  — mirrors the replicated state sync architecture identity rules). The current
  Modelica implementation satisfies that contract only for the restricted
  continuous profile served by `fixed-rk4`; other Modelica programs stay
  authoritative or offline.

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
- **Robotics fast dynamics + control must be realtime-safe** — deterministic,
  fixed-step, often at a control rate distinct from render (the multi-clock hook). So
  robotics is the **forcing function** for Step 3: the fixed-step deterministic solver
  path the promise needs is exactly what a robot's controller/dynamics loop needs. A
  robot is not a special case bolted on — it is the canonical realtime-safe
  custom-solver citizen.
- **External / HIL solvers** (a ROS 2 node, a Copper rate-group, real hardware in
  the loop) plug in as a **Backend** whose `step()` advances an external loop and
  whose ports bridge ROS topics ↔ `SimConnection` wires. This is the
  ROS2/Copper-as-bridge path already in replicated state sync architecture — a robot
  controller running its own solver is just another participant on the wire.
- **Custom solvers stay inside the same contract**: a custom solver behind a
  realtime-safe program must be fixed-step + deterministic (or it isn't predictable); a
  server-authoritative one may be anything (it only streams state). What is duplicated
  across peers is decided by the promise and the wiring, regardless of which solver the
  participant carries.

## 7. Hot-changeable behaviour (incl. vehicle physics at runtime)

Two distinct flavours, different cost:

- **Parameter change** (coefficients, setpoints): cheap. Compile-once + runtime
  parameters (the roadmap item in parallel experiment execution §2b) → feed as
  input wires / `ControlStream` live inputs ([`22-domain-cosim.md`](22-domain-cosim.md)
  control-vs-data plane). No recompile.
- **Structural change** (swap equations / whole model): needs recompile, then
  **hot-swap the compiled stepper** (`BackendCaps.supports_live_swap` reserves this).

How runtime control plays out **depends on whether the client predicts it** — and
vehicle physics is the hard case:

- **Server-authoritative:** either flavour is loose — mutate on the
  server, replication carries the new behaviour to clients. No coordination.
- **Predicted (a vehicle):** a runtime change must be applied **identically
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
a Modelica-described one) is the hot-swap on a predicted body: only once the
selected fixed-step profile validates the replacement and only at a quiesced tick
boundary applied across all peers — never mid-rollback.

## 7. The realtime Modelica profile (how the promise gets kept)

The way to make predicted physics describable in Modelica is **not** to make rumoca's
general adaptive solver deterministic. It is to use a **restricted profile** — a
fixed-step deterministic solver **plus limitations on the model**, with the model
still authored in plain Modelica code. The current profile is implemented by
`lunco_modelica::fixed_step::FixedStepSession`; it is a deliberately small first
profile, not a claim that every Modelica construct is prediction-safe. A model is
admitted only when the backend can establish this profile at construction and is
rejected with a diagnostic otherwise.

**The current special solver:** classical RK4 with exactly four derivative stages
per configured step. It advances only when the caller supplies that exact step
size; it neither adapts, subdivides, nor rounds a step. Determinism comes from the
fixed operation sequence, fixed algebraic solve budget, fixed evaluation order,
and the simulation clock—not from turning an adaptive session's nominal `dt` into
a promise. The profile currently requires continuous, event-free,
external-table-free models. Stability and bounded-state analysis are still model
responsibilities; a future linearly-implicit backend may extend the profile for
stiffer systems.

**The property limitations** (compiler-enforced — the profile's "type system"):

- **Fixed structure** — no variable-structure systems, constant state count.
- **Fixed-step-stable dynamics** — reject systems whose stiffness needs adaptive
  steps to stay stable at the chosen `dt` (or require the linearly-implicit solver).
- **Bounded state** — guards against runaway (the responsive UI mandate
  invariant); a model that can diverge in finite ticks is rejected.
- **No event/discrete semantics yet** — root events, scheduled events, clocks, and
  discrete assignments are rejected. They must acquire explicit tick-quantized
  semantics in a future backend before they can enter prediction.
- **Deterministic evaluation order** — fixed fold order, no wall-clock/random.

This is the same profile **robots** want (§6): a controller / articulated-body
loop is exactly a fixed-step, bounded, deterministic model driving a predicted body.
Robots and vehicles are the two canonical Realtime-profile citizens.

## 8. Staged roadmap

1. **Step 1 — ECS-native, server-authoritative (this doc, §9).** One slow
   domain modelled in Modelica, stepped as an ECS system, output replicated as a
   wire. Proves *all* the asks (declarative physics + realtime + multiplayer +
   hot-param + ECS-native) where no promise is needed, reusing cosim + networking that
   already exist. Lowest risk, highest signal.
2. **Step 2 — the oracle.** A Modelica quarter-car / wheel-friction reference run
   headless via the experiment path, compared against the Rust `suspension_force_mag`
   / `contact_friction` / `drive_force_mag` force laws (now extracted as pure,
   testable functions in `lunco-mobility`). Modelica as **ground truth, out of the
   loop** — would have caught the explicit-Euler limit-cycles immediately. Validates
   the predicted Rust physics without committing to runtime Modelica.
3. **Step 3 — extend the prediction profile (§7).** The initial `fixed-rk4`
   backend now lets a qualified continuous Modelica model honour `realtimeSafe`
   inside the prediction loop. The remaining work is intentionally additive:
   deterministic semantics for events/discrete state and a stiff, bounded-work
   backend. *Parameter* tuning of predicted physics (§7) remains available without
   changing the force-law structure.

## 9. Step 1 scope — an ECS-native, server-authoritative Modelica stepper

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

1. **Authoring** — declare the program + its connections in USD, reusing what
   `lunco-usd-sim` already reads: a `LunCoProgramAPI` prim naming
   `info:sourceAsset`, and native USD connections on its `inputs:`. The
   battery program connects `inputs:load_w` ← rover motor power and exposes
   `outputs:soc` / `outputs:voltage`. It drives no force, so it makes no realtime
   promise. **Zero new Rust to author.**
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

**Explicitly out of scope for Step 1:** the offline oracle (Step 2), structural
hot-swap, and the full Twin / BackendRegistry formalisation. The initial
`fixed-rk4` prediction profile is implemented separately from the
server-authoritative Step 1 path.

## 10. Decision log

1. **Ask one question of every program, don't apply "Modelica everywhere" uniformly:**
   *may it drive a force on a body the client predicts?* That answer also **selects
   the replication mechanism** (duplicate computation / duplicate state / nothing —
   §5), turning "what do we replicate?" into a lookup.
2. **Adaptive solvers may never drive a predicted body.** They are non-deterministic
   across peers and must never enter the client-prediction loop.
2a. **The promise is declared in USD, never inferred.** *Implemented* as
   `uniform bool lunco:program:realtimeSafe` → the `RealtimeSafe` component, read at
   prim-read time and gated at wire-build time on the component's presence (§2a).
   *Designed, not implemented:* `lunco:replication` → the always-on `Replication`
   metadata at spawn, and the load-time promise ↔ solver/caps validation ("rejected on
   conflict"). Unauthored is **not** a promise: such a program may not drive predicted
   physics.
3. **Keeping the promise in Modelica = a Realtime profile (§7): a fixed-step
   deterministic solver + enforced model limitations**, authored in plain Modelica.
   Not "make the adaptive solver deterministic" — constrain the models instead.
   `fixed-rk4` is the first qualified profile; robots and vehicles are its canonical
   citizens. Unsupported semantics remain rejected until a backend defines their
   deterministic meaning.
4. **The Modelica stepper is an ECS citizen**: instance = entity, ports =
   components, state = component, step = system, coupling = `SimConnection` wire,
   replication = the existing wire layer. No bespoke runtime.
5. **Server-authoritative multiplayer is free**: server steps, output wire replicates,
   solo degrades to local with no reconciliation by construction.
6. **Realtime safety = bounded compute**: off-thread stepping, sub-rate,
   step-budget-with-degrade. Never silently stall; a runaway model fails its run.
7. **Solver is a per-participant property, not global** (the `step(dt)` contract +
   `native_solver` cap). This is what makes the system **robotics-ready**: each
   robot/model brings its own solver; external/HIL solvers (ROS 2 / Copper) plug in
   as Backends bridging topics ↔ wires.
8. **Hot-param is cheap (runtime params), hot-structure is a stepper hot-swap**
   (server-authoritative any time; predicted only at quiesced tick boundaries).
9. **Predicted vehicle physics is runtime-controllable now at the parameter level**:
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
- `lunco-mobility/src/lib.rs` — the Rust force laws behind predicted physics Step 2 will validate
