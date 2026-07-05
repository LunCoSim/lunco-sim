# 36 — Comms Connectivity & Sky Visualization

Status: **design / analysis** (2026-07-03). Greenfield — no comms/LOS/sky-body primitives exist yet.

> **Reframed by `38-domains-as-packages.md`:** comms is the first *domain package* (namespace
> `lunco:comms`, connector kinds `rf`/`data`, `CommsLink.mo` synthesis, topology editor, margin
> validation); the multi-layer component of §2 is a *part* in that domain's library. Read doc 38 for the
> organizing principle and doc 37 for the model-synthesis mechanism.

> Related asks, one substrate:
> 1. **Track rover↔Earth/satellite radio connectivity** — geometry-driven line-of-sight + link
>    availability, gated by a **USD flag** and computed by a **connection simulation layer**.
> 2. Make connectivity a **reusable multi-layer component** — 3D + link(RF) + electrical + policy
>    layers, one referenceable USD asset that composes into rovers *and* robots (§2), realized as a
>    cosim subsystem (§4.5).
> 3. **Visualize the sky** — Sun/Earth as correctly-sized distant bodies + a starfield/skybox,
>    **reusing the existing Earth `GlobeLod` model** (§7), coarsest LOD by default in the sandbox.
> 4. Move the Earth/celestial-body **definition into a USD file** now that USD authoring tools exist (§7.1).
>
> All downstream of the *same* celestial-geometry query (where is Earth/the satellite, relative to this
> rover, right now). Build the geometry core once; comms, subsystem, and sky all read it.

---

## 1. What already exists (build on, don't rebuild)

| Capability | Where | Reuse for |
|---|---|---|
| Time-parameterized body positions (Sun 10, EMB 3, Earth 399, Moon 301 + CSV-table custom NAIF ids) | `lunco-celestial` `EphemerisResource.provider.{position,global_position}(id, epoch_jd)` | rover→Earth/sat vector |
| Mission clock spine (`epoch_jd`, TDB) | `lunco-time` `WorldTime.epoch_jd` | the "when" for every query |
| Absolute world position across floating-origin grid | `lunco_core::coords::world_position_seeded(entity,…)` | rover & target world pos |
| Body identity + radii + SOI | `CelestialBodyRegistry` / `BodyDescriptor { radius_m, gm, polar_axis, … }` | **occlusion by body sphere**, horizon |
| Body-fixed rotating frame (rover inherits Moon spin) | `body_rotation_system` (`systems.rs:96`) | Earth rises/sets over lunar day automatically |
| Local up / gravity vector | `LocalGravityField` (`GravityPlugin`) | elevation angle above horizon |
| General world-space raycast | `query("Raycast", {origin,dir,max})` → `{hit,entity,distance,point,normal}` (`lunco-mobility/src/sensing.rs:29`) | terrain-limb LOS occlusion |
| Ray-fan sensor pattern (component→resource→consumers) | `lunco-autopilot` `Clearance`/`ClearanceField` (`lib.rs:108,126`) | template for a `CommsField` |
| USD attr → ECS component projection | `lunco-usd-sim` `process_usd_sim_prims` (`lib.rs:494`), pattern `lunco:...` bool/attrs → `commands.insert(Comp)` | the **USD flag** |
| Scalar → readable port (API `read_port`, rhai `GetPort`, MCP) | `lunco_core::ports::PortRegistry` (`ports.rs:194`) | expose link-budget / connected bool |
| Event edges (connect/disconnect) | `TelemetryEvent` bus (`lunco-core`); rhai `on_event`/`emit` | scripts react without polling |
| Spacecraft placement (billboard + transform from ephemeris) | `Spacecraft` (`lunco-core:245`), `MissionSpacecraft`, `update_spacecraft_position_system` | the "satellite" object |
| USD `DistantLight`→sun `DirectionalLight`, `DomeLight`→ambient scalar | `lunco-usd-bevy/src/light.rs` | sky lighting; DomeLight is the skybox hook |
| **Subsystem = USD sub-prim + Modelica model + port wiring** (no `Subsystem` enum; open/data-driven) | `def Scope "X"` + `lunco:modelicaModel`+`lunco:simWires` → `SimComponent` (`lunco-usd-sim/src/cosim.rs`); pattern doc `34-…md:45-67` | Comms as a subsystem |
| **Fidelity toggle `"comms-degradation"` already registered** | `lunco-core/src/subsystems.rs` `SUBSYSTEMS` allow-list + `SubsystemToggles`; rhai `set_subsystem` | gate comms fidelity, zero new code |
| Earth/Moon body = `GlobeLod` cube-sphere tile globe (blueprint shader), **not a mesh** | `big_space_setup.rs:270-319` + `globe_lod.rs`; knobs = `GlobeLod{max_lod,lod_distance_factor,res}` | reuse for distant Earth; `max_lod:0` = coarse |
| USD authoring from running app | `ApplyUsdOp{AddPrim,SetAttribute,SetRelationship}` + `SaveDocument` (`lunco-usd/src/commands.rs`); serialize via `author::data_to_usda` | move Earth def into `.usda` |

### The real gaps
1. **No line-of-sight / elevation / occlusion / link primitive** anywhere — greenfield.
2. **No Keplerian/orbital-element satellite propagator.** Satellites today = `Spacecraft` whose
   position is *table-interpolated* from a JPL-Horizons CSV (the `other_id` arm,
   `lunco-celestial-ephemeris/src/lib.rs:256`). No `(a,e,i,Ω,ω,ν)` propagation.
3. **No sky rendering of distant bodies from a surface viewpoint** as a curated disk, and **no
   starfield/skybox/atmosphere** — background is a flat clear color (`sandbox lib.rs:414`). Bodies
   *are* real physical-radius spheres in `big_space` (correct angular size by geometry), but there
   is no impostor/curated-sky layer and no stars.
4. **No `CommsLink.mo` Modelica model** — flagged as the biggest cosim gap in `34-…md:148,160`
   (range → data-rate → buffer, "no model yet"). Template = `assets/models/Battery.mo` (SoC-integral
   resource budget).
5. **No celestial-body ↔ USD bridge.** Body spawning (`big_space_setup.rs`) is entirely separate from
   the USD import path; a USD `Sphere` prim yields a *plain StandardMaterial UV sphere*, not the
   blueprint-shader `GlobeLod` globe. Moving the Earth definition into USD needs a new
   `lunco:celestialBody`/`lunco:globeLod` attribute handler (§7.1).

---

## 2. The component model — connectivity as a reusable multi-layer asset

> This is the heart of the request: connectivity must be a **reusable component with many layers**
> (3D, Modelica dynamics, electrical, RF, policy) that later composes into robots and larger
> assemblies. The good news — **USD's composition engine already gives us exactly this**, and the
> shipped Power/Mobility components + the drivetrain variantSet are the working precedent. What's
> missing is a *discipline*: today's components cram every domain onto one prim, which does not scale
> past one solver. The multi-layer form below is what generalizes.

### 2.1 What USD composition already supports here (verified)

The `openusd` PCP engine composes and the loader flattens (all carrying `lunco:` attrs through):

| Arc | Works? | Use for a component |
|---|---|---|
| **`references` (incl. sub-tree `@file@</Prim>`)** | ✅ read+compose; `author_reference()` helper | the primary reuse arc — pull a component asset onto a child prim; per-instance param overrides win (LIVERPS) |
| **`variantSet` / variants** | ✅ read+compose (drivetrain `raycast\|physical` ships) | **fidelity levels** (ideal / link-budget / full) and config, selected per instance |
| **`subLayers` + `inherits`(class)** | ✅ (control-profile pattern ships) | shared defaults across a fleet (a `_CommsDefaults` class) |
| **`over`** | ✅ | sparse per-instance tweaks / runtime overlays |
| **binary `.glb` payload** | ✅ deferred → `lunco:resolvedAsset` (async) | the **heavy 3D mesh** — load it lazily so a schematic/electrical view opens without geometry |
| **nesting (BFS, unbounded)** | ✅ scene→rover→component depth ≥3 already | **robot = arm → gripper + motors ×N + comms**, each a referenced asset |
| `.usda` payload | ⚠️ *eager* (composes like a reference) | not a lazy boundary — keep heavy geometry as `.glb`, not `.usda`, if you need deferral |
| `instanceable` / native prototypes | ❌ dropped by flatten (`PrimPredicate::DEFAULT`) | **not usable today** — many identical antennas = N plain `references` (like six_wheel_rover's 6× wheel), or extend flatten to `DEFAULT_PROXIES` |

Takeaway: **reuse, fidelity variants, fleet defaults, deferred geometry, and deep nesting all work now
by hand-authoring arcs** in `.usda`. Only `references` has an authoring *helper*; variants/payloads/
inherits must be written into the asset text (fine — components are authored assets, not runtime-built).

### 2.2 Anatomy of a multi-layer component

A component is a **referenceable USD asset** (`defaultPrim` = an `Xform`, USD `kind="component"`) whose
children are **one sub-prim per layer/domain**, each binding at most one model — because the runtime is
**one solver per prim** (doc 34 decision; a bare `lunco:modelicaModel` with no `lunco:simWires` is inert
doc, so extra domains *must* be separate prims). The layers of a Comms component:

```usda
def Xform "CommsSystem" (kind = "component") {          # ── the reusable unit; defaultPrim
    # public interface — the component's "connectors" (see §2.3):
    custom string lunco:ports = "rf_out:out, p_draw:out, cmd_in:in, data_out:out"

    def Xform "Geom" {                                  # ── Layer 1: 3D / structure
        custom bool lunco:comms:antenna = 1             #     the physical-antenna flag (§5)
        prepend payload = @lunco-lib://models/hga.glb@  #     heavy mesh → deferred binary payload
        # + collider, mount frame, optional gimbal joint
    }
    def Scope "Link" {                                  # ── Layer 2: RF / link dynamics (Modelica)
        custom string lunco:modelicaModel = "models/CommsLink.mo"   # Friis → data-rate → buffer
        custom string lunco:simWires  = "range_km:u_range, connected:u_up, dataRate:data_out"
        custom string lunco:portEvents = "margin_db<0:comms:loss, margin_db>3:comms:acquire"
    }
    def Scope "Power" {                                 # ── Layer 3: electrical draw (Modelica)
        custom string lunco:modelicaModel = "models/CommsPower.mo"  # TX state → DC watts
        custom string lunco:simWires = "txActive:u_tx, p_draw:p_draw"   # p_draw → vehicle EPS bus
    }
    def Scope "Policy" {                                # ── Layer 4: mode/relay policy (rhai)
        custom string lunco:script = "scripts/comms_policy.rhai"   # handover, duty-cycle, safe-mode
    }
}
```

Layers are wired **internally** by `lunco:simWires` / wire-prims through `PortRegistry`; the RF and
electrical `.mo`s couple to each other and to the rover's EPS/OBC through named ports. The **3D-geometry-
LOS layer is not per-component Modelica** — it's the shared `lunco-connectivity` Rust substrate (§3)
publishing `range_km`/`elevation_deg`/`connected` as *input ports* keyed by the antenna entity, which
the Link `.mo` consumes. This is the house layering exactly: **USD = structure/wiring, Modelica/rhai =
per-vehicle dynamics/policy, Rust = reusable substrate never authored per vehicle** (`33-…md:26-29`).

### 2.3 The public port interface (the composability crux)

For components to snap together like LEGO, each must expose a **small, named, typed port interface** —
its SSP *connectors* — so an assembly can wire to `CommsSystem.rf_out` / `.p_draw` **without knowing the
internals**. Today ports are discovered per-backend but a component does not *declare its public
surface*. Recommended addition: a `lunco:ports` manifest on the component root (above) that registers
those names as the component's boundary; internal prim ports stay private. This is the one genuinely new
substrate piece the component model needs, and it is small (a manifest attr + a registry entry). The
`PortType {Force,Kinematic,Electrical,Thermal,Signal}` enum already exists (`lunco-core/ports.rs`) — use
it to *tag* interface ports (cosmetic today, but it makes a comms `rf_out` vs a power `p_draw`
self-describing for tooling and connection validation).

### 2.4 Electrical: causal ports now, acausal networks later (decision point)

"Electrical & so on" forces a real choice, because two unrelated things both exist today:
- **`rel lunco:epsBus` / `lunco:powerInput`** — an author-side electrical *topology graph* on the
  shipped components. **No Rust reads it** — pure forward-looking schema, zero runtime behavior.
- **`SimConnection` wires** — directed scalar copies (`end.in = start.out·scale+offset`, SSP) between
  Modelica I/O via `PortRegistry`. This is the *live* coupling. There is **no acausal effort/flow
  (voltage/current) electrical network at the Rust level** — acausal `connect()` exists only *inside* a
  single `.mo` (rumoca flattens it).

Two ways to model the electrical layer, and they don't mix cleanly:

| | **Causal scalar ports (recommended now)** | **Acausal physical network (later/optional)** |
|---|---|---|
| Electrical coupling | component exposes `p_draw` (W) out-port → a bus/`Battery.mo` integrates Σloads | each component exposes a Modelica electrical *connector* (V,I); assembly `.mo` `connect()`s them |
| Composition | **per-component encapsulation; SSP-native; works today** | one electrical `.mo` spanning many components — breaks per-component encapsulation |
| Fidelity | power *budget* (energy, SoC, brown-out) — enough for mission sim | true circuit (bus voltage sag, load sharing) |
| Tooling | USD wires, `PortRegistry` | cross-file Modelica composition (rumoca), not USD wires |

**Recommendation (refined by doc 37 — read it for the full treatment):** the clean rule is **acausal
*within* a domain, causal *across* domains.** So:
- The comms component contributes its electrical draw as a **causal `p_draw` boundary port** (encapsulated,
  ships now) — that is the *cross-domain* coupling, and it's correct.
- The rover's **electrical network itself** (battery + bus + all loads incl. this `p_draw`) is **one
  acausal `Electrical.mo`** — a single DAE with real Kirchhoff `connect()`, which rumoca **does support
  today** (MSL `Electrical.Analog`, effort/flow flatten). This is *not* "later" — it is the right home
  for the circuit, and it can be **synthesized at runtime from the composed USD components** (the inert
  `rel lunco:epsBus` topology *is* the netlist). See `37-model-synthesis-and-multidomain-composition.md`.

So the comms component stays a sealed unit with a signal interface (`p_draw` out), *and* the electrical
physics is faithful — they don't conflict once the electrical network is one synthesized DAE rather than
N co-simulated component circuits. A global *co-sim* electrical solver (N circuits, scalar-wired) is the
thing to avoid (breaks Kirchhoff + adds lag); a single synthesized electrical DAE is the thing to build.

### 2.5 Fidelity as a variantSet (+ the runtime toggle)

Map the layers onto a `variantSet "fidelity"` on the component — the *exact* shape of the shipping
drivetrain `raycast|physical` variant:

```usda
def Xform "CommsSystem" (kind="component") {
    variantSet "fidelity" = {
        "ideal"     { over "Link" { } over "Power" { } }        # geometry LOS only → connected bool
        "linkbudget"{ over "Link" (lunco:modelicaModel="models/CommsLink.mo") { } }   # + Friis/buffer
        "full"      { over "Link" {…} over "Power" {…} over "Therm" {…} }             # + electrical + thermal
    }
    prepend variantSets = "fidelity"
}
```

An assembly selects per instance (`variants = { string fidelity = "linkbudget" }`), and the runtime
`SubsystemToggles::enabled("comms-degradation")` (already registered, §4.5) gates the *degradation*
behavior on top. Authoring granularity (USD variant, per twin) and runtime granularity (toggle, per
session) compose.

### 2.6 How this generalizes to robots

A robot is the same mechanism nested: `def Xform "Rover" (kind="assembly")` → `references` an
`arm.usda` (itself `kind="assembly"` → `references` `gripper.usda` + `joint_motor.usda` ×N +
`CommsSystem.usda`). Each referenced component brings its own layer sub-prims + its `lunco:ports`
interface; **assembly-level wire-prims** connect subcomponent ports (comms `p_draw` → EPS bus,
GNC `pointing_cmd` → antenna gimbal). Nesting is unbounded (BFS loader), per-instance overrides work at
every level, and identical parts are N `references` (until native instancing is wired). The connectivity
component is thus the *first worked example* of the general "build complex things from reusable
multi-layer subcomponents" pattern — do it right here and robots fall out of the same rules.

> **Net:** the composition capability is already present (references + variants + nesting + deferred
> `.glb` + `over`). The connectivity component needs only: (1) the `lunco-connectivity` Rust substrate
> (§3), (2) `CommsLink.mo` + `CommsPower.mo` (§4.5), (3) the small `lunco:ports` public-interface
> manifest (§2.3), (4) the `lunco:comms:antenna` flag handler (§5). Everything else is authoring.

---

## 3. Layer A — Celestial geometry core (shared)

One new crate `lunco-connectivity` (or a module in `lunco-celestial`). Pure geometry, no rendering,
no RF. Everything else reads it.

**Inputs:** `EphemerisResource`, `WorldTime.epoch_jd`, `CelestialBodyRegistry`, `world_position_seeded`,
`LocalGravityField`.

**Core query** (write this — none exists):

```rust
/// Geometric relationship from an observer (rover) to a target (Earth / satellite) right now.
struct SightLine {
    target_id: i32,          // NAIF id or spacecraft ephemeris_id
    range_m: f64,
    dir_world: DVec3,        // unit, engine frame
    elevation_rad: f64,      // above local horizon (vs LocalGravityField up); <0 = below horizon
    azimuth_rad: f64,
    occluded_by: Occlusion,  // None | Body(i32) | Terrain(Entity)
}

enum Occlusion { None, Body(i32), Terrain(Entity) }
```

**Algorithm per (observer, target):**
1. `obs = world_position_seeded(rover,…)`; `tgt = ecliptic_to_bevy(global_position(target_id,jd) - global_position(ref,jd))`
   (template: `systems.rs:209-222`).
2. `dir = (tgt-obs).normalize()`, `range = |tgt-obs|`.
3. `up = LocalGravityField` up; `elevation = asin(dir·up)`; azimuth from tangent-plane basis.
4. **Occlusion, two-stage (cheap→expensive):**
   - **Analytic body-sphere test** (primary, no colliders): for each body in `CelestialBodyRegistry`,
     ray-vs-sphere on the segment obs→tgt using `radius_m`. This is what handles the **lunar limb /
     horizon** — the explorer flagged that the far globe often has *no* physics collider, so a
     raycast alone won't occlude beyond the horizon. Analytic sphere test is mandatory, correct, and
     cheap. (Elevation < 0 is the first-order horizon proxy for the observer's own body.)
   - **Terrain raycast** (optional refinement, near field only): `query("Raycast", {origin:obs,
     dir, max:range})` to catch crater rims / local relief within a few km. Skip past a range cap.

**Output surface:** `Resource CommsField(HashMap<Entity, Vec<SightLine>>)` — mirror the
`ClearanceField` pattern (decoupled producer/consumer, `lunco-autopilot/lib.rs:126`). Recompute at a
throttled cadence (comms geometry changes on minutes-to-hours scale; 1–10 Hz is ample, gate like the
30 FPS horizon timer).

---

## 4. Layer B — Connection simulation (the "link" on top of geometry)

Geometry says *can these two see each other*. The connection layer decides *is there a usable link*
and *how good*. Keep policy in data/rhai per the house style (open registries, not Rust taxonomies).

**Minimum viable (availability):** `connected = elevation ≥ mask_angle && occlusion == None && range ≤ max_range`.

**Link-budget tier (optional, later):** classic Friis — `Prx = Ptx + Gtx + Grx − FSPL(range,freq) − losses`;
`FSPL = 20·log10(range) + 20·log10(freq) + 32.44`. Compare `Prx` vs receiver sensitivity → margin (dB).
Parameters (`tx_power`, `gain`, `freq`, `sensitivity`, `mask_angle`) are **per-antenna data**, authored
in USD (below) or a rhai table — not hardcoded.

**Relay/routing (optional, later):** if direct rover→Earth is occluded but rover→satellite and
satellite→Earth are both clear, the link routes via the satellite. This is a tiny graph reachability
pass over the per-pair `SightLine`s — do it in rhai policy, not Rust, so mission scenarios own the
topology.

**Outputs (two channels, per existing substrate):**
- **Continuous scalars → `PortRegistry`** so `read_port`/rhai `GetPort`/MCP see them without polling
  the ECS. Easiest path: attach a `PhysicalPort`/`DigitalPort` to the comms entity (already backed,
  zero new backend code, `ports.rs`). Or a custom `PortBackend` closure over `CommsLink` (template:
  `AvianPort`, `lunco-cosim/src/ports.rs:42`). Bool→`0.0/1.0`. Ports: `link_margin_db`, `range_km`,
  `elevation_deg`, `connected`.
- **Edges → `TelemetryEvent`** (`emit("comms:acquire"/"comms:loss", …)`) so rhai `on_event` reacts to
  AOS/LOS (acquisition/loss of signal) without polling. Mirror `register_collision_telemetry`
  (`sensing.rs:273`).

This makes connectivity observable to: the API (`read_port`), rhai scenarios (`GetPort`/`on_event`),
MCP (`read_port` tool), and the cosim graph (a Modelica FSW model can consume `connected` as an input
port) — all through channels that already exist.

---

## 4.5 Connectivity as a first-class subsystem (USD Scope + Modelica)

The codebase has **no `Subsystem` enum / no closed catalog of kinds** — a subsystem is an *emergent*
structure: a **USD sub-prim** bound to a **Modelica model**, wired through the **f64 port substrate**
(SSP-shaped). This is exactly how Power/Propulsion are already authored (`assets/components/power/*.usda`
+ `assets/models/Battery.mo`), and the doc-34 decision explicitly says each domain (GNC/Power/Thermal/
**Comms**) is its own child `Scope` prim under the vehicle — *not* N solvers on one entity
(`34-…md:45-67`). So "introduce a connectivity subsystem" means **author one prim + one `.mo`**, no new
Rust taxonomy.

Two halves, cleanly separated:

- **Geometry & availability (Layers A/B above)** — the *environment-facing* half: where is Earth, is
  there LOS, elevation, range. Lives in `lunco-connectivity` (Rust substrate, reusable, never
  per-vehicle). Produces `SightLine`/`CommsField` and publishes `range_km`/`elevation_deg`/`connected`
  as **input ports** other models can read.
- **Link dynamics (this section)** — the *vehicle-facing* half: a `CommsLink.mo` Modelica model
  authored per vehicle as a subsystem. Consumes the geometry ports, produces data-rate / buffer /
  energy-per-bit dynamics. This is the "Comms subsystem" proper.

**Authoring pattern** (mirror the Power sub-prim, `34-…md:57`):

```usda
def Xform "Rover" {
    def Scope "Comms" (prepend apiSchemas = ["LunCoCommsAPI"]) {
        custom string lunco:modelicaModel = "models/CommsLink.mo"   # dynamics
        # wire geometry input ports (from lunco-connectivity) → model inputs,
        # and model outputs → this entity's telemetry/consumer ports:
        custom string lunco:simWires = "range_km:u_range, connected:u_up, dataRate:tlm_rate"
        # threshold edges → TelemetryEvent (AOS/LOS on the modelled margin):
        custom string lunco:portEvents = "margin_db<0:comms:loss, margin_db>3:comms:acquire"
    }
    def Xform "HGA" { custom bool lunco:comms:antenna = 1 ; ... }   # the physical antenna (§5)
}
```

Translation is automatic: `process_usd_cosim_prims` (`lunco-usd-sim/src/cosim.rs:113`) loads the `.mo`,
compiles it, wraps it as a `SimComponent`, and turns `lunco:simWires` into `SimConnection`s
(SSP `value = src·scale + offset`). The model's I/O then lives in `PortRegistry` alongside everything
else — reachable by API/rhai/MCP and by *other* subsystems (e.g. a Power model consuming `comms:txPower`).

**`CommsLink.mo` (new — the doc-34 gap).** Template = `assets/models/Battery.mo` (state-integral
resource budget). Sketch:

```modelica
model CommsLink "geometry-gated link + data buffer"
  input  Real u_range   "range [km] (from lunco-connectivity port)";
  input  Real u_up      "1 if geometric LOS+elevation OK, else 0";
  parameter Real txPower_dBW = 10, gain_dBi = 25, freq_Hz = 2.2e9, sens_dBW = -140;
  parameter Real genRate = 5.0 "science data generated [Mb/s]";
  Real margin_db, dataRate, buffer(start=0) "on-board buffer [Mb]";
equation
  margin_db = txPower_dBW + gain_dBi
            - (20*log10(max(u_range,1e-3)) + 20*log10(freq_Hz) + 32.44) - sens_dBW;
  dataRate  = if (u_up > 0.5 and margin_db > 0) then min(genRate, 50) else 0;  // downlink when up
  der(buffer) = genRate - dataRate;                                            // fills when LOS lost
end CommsLink;
```

This is the classic *range→data-rate→buffer* model doc-34 asked for: when the geometry port drops LOS,
`dataRate→0` and the buffer integrates up (data backlog during occultation) — a physically meaningful
consequence of losing the link, not just a boolean.

**Fidelity toggle already exists.** `"comms-degradation"` is already in the `SUBSYSTEMS` allow-list
(`lunco-core/src/subsystems.rs`) with a `SubsystemToggles` resource and a rhai `set_subsystem(name,on)`
verb. Gate the link model on `SubsystemToggles::enabled("comms-degradation")` for progressive fidelity
(off = always-connected ideal link; on = geometry-gated + buffer dynamics). Zero new registration.

> **Layering recap** (`33-…md:26-29`): USD = structure + wiring, Modelica/rhai = subsystem dynamics,
> Rust = reusable parameterized substrate (never bespoke per vehicle). Comms fits this cleanly —
> `lunco-connectivity` is the Rust substrate, `CommsLink.mo` is the per-vehicle dynamics, the `Comms`
> Scope prim is the structure.

---

## 5. The USD flag ("celestial mechanics / comms turned on in USD")

Follow the canonical `lunco:` custom-attribute → component pattern (`lunco-usd-sim` `process_usd_sim_prims`,
`lib.rs:494-550`; `RangeSensor` is the closest template — presence bool + companion param attrs).

Author on the rover's antenna prim:

```usda
def Xform "HGA" {
    custom bool   lunco:comms:antenna    = 1        # flag → insert CommsLink component
    custom string lunco:comms:target     = "earth"  # "earth" | NAIF id | rel to a Spacecraft prim
    rel           lunco:comms:targetPrim  = </World/Sats/Relay1>   # for satellite targets
    custom double lunco:comms:maskAngle   = 5.0      # deg above horizon
    custom double lunco:comms:txPower     = 10.0     # dBW   (optional link-budget tier)
    custom double lunco:comms:gain        = 25.0     # dBi
    custom double lunco:comms:freq        = 2.2e9    # Hz (S-band)
    custom double lunco:comms:maxRange    = 4.5e8    # m
}
```

Projection branch (new, in `process_usd_sim_prims`, modeled on the RangeSensor branch):

```rust
if reader.prim_attribute_value::<bool>(&sdf_path, "lunco:comms:antenna").is_some() {
    let target   = reader.read_token(&sdf_path, "lunco:comms:target");
    let mask     = reader.prim_attribute_value::<f64>(&sdf_path, "lunco:comms:maskAngle").unwrap_or(5.0);
    // … read the rest, resolve rel target prim → ephemeris_id …
    commands.entity(entity).insert(CommsLink { target, mask_deg: mask, /* … */ });
}
```

**Celestial-mechanics-as-flag.** Distinguish two flavors:
- **Per-object celestial participation** — e.g. `custom bool lunco:celestial:body = 1` +
  `int lunco:celestial:naifId = 399` promotes a prim into the ephemeris/registry (a *satellite* prim
  gets `lunco:celestial:ephemerisId` + a CSV/orbit source). This is the clean way to author "this
  prim is a comms relay orbiter" and have the connectivity layer find it by NAIF id.
- **Scene-level enable** — a `LuncoScenario`/root-prim bool `lunco:celestial:enable = 1` to switch the
  ephemeris-driven celestial plugins on for a given twin (today they're always-on defaults). Optional.

> Note a small existing gap the explorer surfaced: terrain **georeference lat/lon is caller-supplied,
> not yet a `lunco:` USD attr** (`lunco-terrain-core/src/source.rs:97`). If you want the rover's
> lat/lon (needed for a real horizon mask and for authoring fixed ground stations), add
> `lunco:geo:lat/lon` following this same pattern — it's the natural companion to the comms flag.

---

## 6. The satellite object (Earth-orbit / lunar relay)

Today a satellite = `Spacecraft { ephemeris_id, reference_id, … }` positioned by table-interpolated
Horizons CSV (`missions.rs` + the `other_id` ephemeris arm). Two paths:

- **Now (zero engine work):** author the relay as a mission/`Spacecraft` prim with a Horizons CSV for
  its orbit. Give it a NAIF-style custom id; the connectivity layer targets it by id. Good enough to
  demo rover→relay→Earth handover with a *real sampled* orbit.
- **Later (propagator):** add a Keplerian/SGP4 source implementing `EphemerisProvider::position` for
  custom ids (elements authored as `lunco:orbit:{a,e,i,raan,argp,nu,epoch}` USD attrs). This is the
  only way to get *arbitrary* user-defined orbits without a Horizons export. Slots in behind the
  existing `EphemerisResource` trait object — no consumer changes.

Recommend starting with the CSV path (proves the whole pipeline) and treating the propagator as a
follow-on once the geometry+link+USD-flag spine is validated.

---

## 7. Sky visualization

Current state: bodies are **real physical-radius spheres/LOD terrain in `big_space`** viewed through
the floating origin, so Sun/Earth **already hold correct angular size by geometry** — but there is
**no starfield, no skybox, no env map, no atmosphere** (pure-black clear color, intentional for the
airless Moon), and **no curated sun/Earth disk** decoupled from the physics sphere.

### What USD offers here
USD's environment vocabulary is **UsdLux**, and the import layer already reads it (`lunco-usd-bevy/src/light.rs`):

| USD prim | Standard meaning | Status here | Sky use |
|---|---|---|---|
| `DistantLight` | infinitely-far directional (the Sun) + `inputs:angle` (0.53°) | **mapped** → `DirectionalLight` + `SunAngularDiameter` | already drives sun lighting & shadow penumbra |
| `DomeLight` (+ `inputs:texture:file`) | image-based environment / **skybox**; the canonical USD way to author a sky/star background as an HDRI/cubemap | **partially mapped** — currently collapsed to a scalar ambient only; **texture ignored** | **this is the skybox hook**: promote `DomeLight.texture:file` → Bevy `Skybox` + `EnvironmentMapLight` |
| `SphereLight` / `DiskLight` | area lights | `SphereLight`→Point/Spot | (local, not sky) |

**USD has no first-class "star catalog" or "planet-in-sky" prim.** The idiomatic USD approach to a
star/space background is a **`DomeLight` with an equirectangular starfield texture** (an HDRI of the
sky). So the USD-native answer to "stars + sky" is: **finish the `DomeLight` texture path** into an
actual environment map instead of the current ambient-scalar collapse. The explorer confirmed this is
a clean promotion point (`light.rs:184-190`).

### Reuse the existing Earth model (don't build a new one)

Earth is **not** a mesh — it's a `CelestialBody { ephemeris_id:399, radius_m:6371e3 }` whose surface is
the `GlobeLod` blueprint-shader cube-sphere tile globe (`big_space_setup.rs:270-319`; `GlobeLod {
radius_m, surface_grid, material:earth_blueprint, res:32, max_lod:8, lod_distance_factor:2.0 }` +
`GlobeTiles`). It is already positioned at 399's ephemeris coordinate in `big_space`, so **from a lunar
surface camera it should render at correct angular size for free** — reuse it directly, no impostor
needed in the common case.

**Coarsest LOD by default (sandbox).** LOD depth is the `GlobeLod.max_lod` field, selected per-face by
camera-distance vs tile arc-size in `update_globe_lod` (`globe_lod.rs`). There is **no min-lod / pin
knob today** — coarsest = the 6 cube-face roots. Two ways to get "coarse by default":
- **Quick:** set `GlobeLod.max_lod = 0` at the two insert sites (`big_space_setup.rs:316` Earth, `:389`
  Moon) → each body is a static **6-tile cube-sphere**, never subdivides. Cheapest possible globe;
  perfect for a distant Earth-in-sky and for a low-spec sandbox default.
- **Better:** add a `min_lod`/`pin_coarsest` field to `GlobeLod` consulted in `subdivide_face`, and a
  sandbox config (or `SubsystemToggles`-style flag) that sets it. Lets the sandbox default to coarse
  while a "high detail" toggle raises `max_lod` for close-up work. Recommended — one field + one early
  return.

> ⚠️ Known blocker: `globe_lod.rs:50-68` carries a `TODO(globe-invisible)` — globe tiles reportedly
> render **black** in dev. Whatever coarse-LOD default we ship must be validated against that bug; a
> coarse 6-tile globe is also the simplest repro to fix it on.

### Recommended sky work (Bevy 0.18)
1. **Starfield / space background** — `DomeLight`-authored equirect texture → Bevy `Skybox` +
   `EnvironmentMapLight` (needs `.ktx2`/cubemap; convert the equirect at import). Purely additive; no
   new USD schema — reuse the existing `DomeLight` prim, just honor its `texture:file`.
2. **Sun disk** — already an emissive ico-sphere at true radius (`big_space_setup.rs:194`), decoupled
   from the light. Correct angular size already. If it reads too small/large, that's a tuning/bloom
   issue, not missing geometry.
3. **Earth-in-sky** — reuse the `GlobeLod` Earth above. It does **not** currently render as a visible
   body from the surface viewpoint (only a dim Earthshine fill light exists) — **verify** the surface
   camera's grid frame actually draws body 399's tiles (they may cull at range, and the globe-invisible
   bug may bite). If tiles cull at that distance, the cheap fix is `max_lod:0` (always-present 6-tile
   globe) rather than a separate impostor. Only if that's still too heavy, fall back to a
   billboard/impostor disk sized to `SightLine.range` (angular diameter `2·atan(R_earth/range)`) — the
   *curated fixed-angular-size circle* the user described. Prefer reusing the real globe.
4. **Atmosphere** — none, and correctly so for the Moon. If an Earth-side or transit view ever needs
   it, Bevy 0.18 has a built-in `Atmosphere` (Nishita); out of scope here.

> Payoff of doing sky via the geometry core: the same `SightLine.dir/range` that decides *connected*
> also places the Earth disk and points the antenna gizmo — one source of truth for "where is Earth in
> the sky," shared by physics-of-links and pixels-in-sky.

## 7.1 Moving the celestial-body definition into USD

Today the body list is a **hardcoded Rust `vec!`** (`registry.rs default_system()`, 5 bodies) and the
Earth/Moon spawn **bypasses several registry fields with inline literals** (name/id/radius hardcoded at
`big_space_setup.rs:270-288`; texture hardcoded `cached_textures://earth.png` at `:151`, ignoring
`BodyDescriptor.texture_path`). There is **no celestial↔USD bridge** — a plain USD `Sphere` prim
imports as a static StandardMaterial sphere, *not* the `GlobeLod` globe. So moving Earth into USD is a
clean, worthwhile refactor but it is **new work**, not a config change.

**Approach — a `lunco:celestialBody` schema + handler:**
```usda
def Xform "Earth" (prepend apiSchemas = ["LunCoCelestialBodyAPI"]) {
    custom int    lunco:celestial:naifId       = 399
    custom double lunco:celestial:radius        = 6371000
    custom double lunco:celestial:gm            = 3.986e14
    custom double lunco:celestial:rotRatePerDay = 6.30
    custom asset  lunco:celestial:texture       = @textures/earth.png@
    custom int    lunco:globeLod:maxLod         = 0     # coarse by default; raise for detail
    custom int    lunco:globeLod:res            = 32
}
```
Add a handler in the USD projection (`lunco-usd-bevy sync_usd_visuals` or `lunco-usd-sim
process_usd_sim_prims`) that, on seeing `lunco:celestial:naifId`, inserts `CelestialBody` + `GlobeLod` +
`GravityProvider`/`SOI`/`Collider` — i.e. the exact bundle `big_space_setup.rs` spawns today, but
data-driven from the prim. Author it via the existing `ApplyUsdOp{AddPrim,SetAttribute}` + `SaveDocument`
pipeline; serialize with `author::data_to_usda`.

**Sequencing caveat:** bodies live in the `big_space` grid hierarchy (inertial anchor Grid + rotating
body + surface Grid). A USD-authored body must slot into that hierarchy, so the handler needs to run in
(or hand off to) the celestial big-space setup rather than the generic prim→entity path. Recommend a
**two-step migration**: (1) first move the *parameters* to USD (`registry` reads a USD `def Scope
"CelestialConfig"` instead of the Rust `vec!`) — low risk, unlocks authoring radius/texture/LOD/epoch
per twin; (2) later move the *spawn* itself behind the `lunco:celestialBody` handler. Step 1 alone gives
you "Earth defined in USD" and the coarse-LOD-by-default knob without touching the fragile big_space
wiring.

---

## 8. Build order (thin vertical slice first)

**Track 1 — connectivity**
1. **Geometry core** (`lunco-connectivity`): `SightLine` + analytic body-sphere occlusion + elevation,
   reading `EphemerisResource`/`world_position_seeded`. `CommsField` resource. → *rover knows where
   Earth is and whether the Moon occludes it.* (No RF, no USD yet.)
2. **USD flag** (`lunco:comms:antenna` + companions) → `CommsLink` component via `process_usd_sim_prims`.
   → *authorable per rover.*
3. **Availability link** (`connected` from elevation+occlusion+range) → `PortRegistry` scalar +
   `TelemetryEvent` AOS/LOS edges. → *observable via `read_port`, rhai `on_event`, MCP.*
4. **Comms subsystem** (§4.5): author `def Scope "Comms"` + `assets/models/CommsLink.mo` (template
   `Battery.mo`), wire geometry ports → model → buffer dynamics; gate on `SubsystemToggles::enabled(
   "comms-degradation")` (already registered). → *loss of link now integrates a data backlog, not just a bool.*
5. **Satellite target** via Horizons-CSV `Spacecraft` + relay reachability in rhai. → *rover→relay→Earth.*

**Track 2 — sky / Earth model** (independent, can run in parallel)
6. **Coarse-LOD default**: add `min_lod`/`pin_coarsest` to `GlobeLod` (or set `max_lod:0`) + sandbox
   default; fix the `globe-invisible` black-tile bug on the 6-tile globe. → *cheap always-present Earth/Moon.*
7. **Earth-in-sky**: verify the surface camera renders body 399's globe; reuse it (impostor disk only if it culls).
8. **Starfield**: honor `DomeLight.texture:file` → `Skybox`/`EnvironmentMapLight`.
9. **Earth def → USD** (§7.1): step 1 = registry reads a USD `CelestialConfig` scope (params, incl.
   LOD/epoch); step 2 (later) = full `lunco:celestialBody` spawn handler.

**Later**: Friis link-budget margin; Keplerian/SGP4 propagator behind `EphemerisProvider`; Bevy `Atmosphere` for Earth-side views.

Each step is independently demoable and rides existing substrates (ports, telemetry, USD-sim projection,
cosim wiring, ephemeris, raycast, GlobeLod, DomeLight). No new bespoke buses, no Rust taxonomies —
**structure in USD, dynamics in Modelica, policy in rhai, reusable substrate in Rust.**
```
