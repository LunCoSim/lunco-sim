# 28 — Modelica Realtime Physics (declarative custom physics, networked)

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
  work hardened — see [[project_steering_jitter_dac_determinism]]) needs **fixed-step
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

## 3. Realtime budget

Adaptive implicit solvers can blow a frame budget on stiff systems — we have
already hit `BDF step too small` and worker OOM on `RoverThermalSystem` /
`AbdulezerPair` (see [[project_rumoca_main_bump_solver_regressions]],
[[feedback_app_must_never_stall]]). Realtime therefore needs a **bounded-compute
contract**, independent of tier:

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

## 5. Multiplayer mapping, per tier

- **Tier B** — server runs the authoritative stepper; output ports replicate to
  clients as wires over the existing networking channel (D7: gated behind the
  `networking` feature; in solo the wire is local and there is no replication —
  the architecture degrades to single-player *by construction*, matching
  [[project_predict_own_reconciliation]]'s "solo reconcile is a structural no-op").
  Clients render received state; they never integrate it. No determinism needed.
- **Tier A** — requires (1) a rumoca **fixed-step deterministic codegen** path
  and (2) a determinism contract (same fold/step order on every peer, integer
  `SimTick` clock, no `Date::now`/`Math::random` — mirrors the
  [[project_networking_plan]] identity rules). Until both exist, Tier-A physics
  stays in deterministic Rust (avian + the mobility force laws), with Modelica
  used only as an **offline oracle** (§6, Step 2).

## 6. Hot-changeable behaviour

Two distinct flavours, different cost:

- **Parameter change** (coefficients, setpoints): cheap. Compile-once + runtime
  parameters (the roadmap item in [[project_parallel_experiments]] §2b) → feed as
  input wires / `ControlStream` live inputs ([`22-domain-cosim.md`](22-domain-cosim.md)
  control-vs-data plane). No recompile. Works mid-run in every tier.
- **Structural change** (swap equations / whole model): needs recompile, then
  **hot-swap the compiled stepper**. Clean in Tier B (replace the server-side
  integrator; replication continues uninterrupted — `BackendCaps.supports_live_swap`
  already reserves this). In Tier A it changes the determinism contract, so only
  at a quiesced tick boundary, never mid-rollback.

## 7. Staged roadmap

1. **Step 1 — Tier B, ECS-native, server-authoritative (this doc, §8).** One slow
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
3. **Step 3 — Tier A in the loop (the hard ask).** Only if fast dynamics must be
   *computed* by Modelica under prediction: invest in rumoca fixed-step
   deterministic codegen + the determinism contract. Highest risk; do last, once
   1–2 have shown value.

## 8. Step 1 scope — ECS-native Tier-B Modelica stepper

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
   → worker step), gated on `TimeWarpState::is_running()` and sub-rated to ~10 Hz
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
  survives — the [[feedback_app_must_never_stall]] invariant).

**Explicitly out of scope for Step 1:** any Tier-A model, rumoca fixed-step
codegen, the offline oracle (Step 2), structural hot-swap, the full Twin /
BackendRegistry formalisation.

## 9. Decision log

1. **Classify physics by networking role, not uniformly "Modelica everywhere."**
   Tier A (predicted) ≠ Tier B (replicated) ≠ Tier C (local).
2. **Adaptive solvers are for Tier B only.** They are non-deterministic across
   peers and must never enter the client-prediction loop (Tier A).
3. **Tier A needs fixed-step deterministic codegen** before any Modelica model
   can be predicted. Until then Tier-A physics stays in deterministic Rust;
   Modelica serves Tier A only as an offline oracle.
4. **The Modelica stepper is an ECS citizen**: instance = entity, ports =
   components, state = component, step = system, coupling = `SimConnection` wire,
   replication = the existing wire layer. No bespoke runtime.
5. **Tier-B multiplayer is free**: server steps, output wire replicates, solo
   degrades to local with no reconciliation by construction.
6. **Realtime safety = bounded compute**: off-thread stepping, sub-rate,
   step-budget-with-degrade. Never silently stall; a runaway model fails its run.
7. **Hot-param is cheap (runtime params), hot-structure is a stepper hot-swap**
   (Tier B any time; Tier A only at quiesced tick boundaries).
8. **Step 1 reuses existing cosim + networking**, adds only state-as-component +
   one replicated output — no Twin/BackendRegistry refactor as a prerequisite.

## See also

- [`22-domain-cosim.md`](22-domain-cosim.md) — the master loop, `SimConnection`, USD wires
- [`14-simulation-layers.md`](14-simulation-layers.md) — Participants-are-entities, `BackendCaps`
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica/rumoca specifics + `output` convention
- [`../../crates/lunco-networking/DECISIONS.md`](../../crates/lunco-networking/DECISIONS.md) — D1–D7, SimTick, wire-only gating
- `lunco-mobility/src/lib.rs` — the Rust Tier-A force laws Step 2 will validate
