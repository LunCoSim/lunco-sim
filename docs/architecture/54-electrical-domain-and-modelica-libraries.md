# 54 — The Electrical Domain, and Modelica Libraries from USD

> Status: Active · Audience: contributors on cosim, USD assets, and the Modelica library
> Builds on: [20 — Modelica domain](20-domain-modelica.md), [22 — cosim](22-domain-cosim.md),
> [38 — domains as packages](38-domains-as-packages.md)

Two things, one worked example: how a physical subsystem is modelled across the three
planes, and how the Modelica library it depends on reaches the compile session. This doc
leads with **why** each choice was made, because every one of them replaced a plausible
alternative that is wrong for a reason worth remembering.

## 1. The split: why a number lives where it lives

A part lives on three planes:

- **USD assembles** — what exists, where it sits, what it is bolted to, and every
  parameter *value*.
- **Modelica is the maths** — anything that is an equation.
- **rhai is the behaviour** — when to shed a load, where to drive.

**Why not put the electrical numbers on the USD prim?** Because the schema's own header
forbids it, and the forbidding is principled: *a program's parameters (a gain, a capacity)
are ports, not schema properties.* The deeper reason is falsifiability. A quantity with an
equation behind it can be *checked* — the simulation either balances or it does not. An
attribute authored on a prim that no equation consumes can only be *trusted*, and trust
fails silently. A panel that states `800 W` beside `72 m²` of 32%-efficient cells (which
imply forty times that) has no equation to catch the contradiction; nobody reads the
number, so nothing objects. Moving the value into a model turns a silent lie into a
checkable claim. That is the whole reason the split exists — not tidiness.

## 2. USD assembles components; the runtime projects a composite model

Each physical component applies `LunCoProgramAPI` and names its reusable Modelica class.
Compiler-network members explicitly author
`info:implementationSource = "sourceAsset"` and a `.mo` `info:sourceAsset`; built-in
`info:id` programs and inline `info:sourceCode` programs are valid elsewhere, but are not
Modelica compiler inputs.
Its causal boundary uses `inputs:`/`outputs:`; its acausal Modelica connector members use
`connectors:`. Ordinary USD property connections author topology. There is no electrical
USD schema and no exposed `Pin` prim. An ordinary network `Scope` applies the standard
multiple-apply `CollectionAPI:components`; that collection is the explicit working set
for one projected Modelica root model. The built-in network projector derives a
deterministic program graph from that working set, partitions connected subgraphs into
named composite units, and emits those units below the root. The Scope remains one
runtime participant with one public boundary; generated child models are not extra ECS
entities or an alternate wiring path.

A circuit is not directional. It is acausal, and Modelica exists precisely to express
that: a `Pin` with a `flow` variable, connected with `connect()`, makes the tool write
Kirchhoff's current law itself.

```modelica
connector Pin
  Real v;            // shared at a node — every connected pin sees one voltage
  flow Real i;       // summed to zero at every node — Kirchhoff, written by the tool
end Pin;
```

**Membership is what makes a facet ours.** A prim listed in
`collection:components:includes` is compiled INTO the generated model, and for that reason
gets no solver of its own and no runtime wires on its `inputs:` — both would duplicate what
the wrapper's equations already do. A facet that declares `connectors:*` and belongs to NO
collection cannot be solved at all (its pins only mean something inside a `connect()` set)
and says so at load rather than sitting inert.

**The network boundary is derived from typed USD structure.** The presence of
`CollectionAPI:components` identifies the Scope as a Modelica network. Its composed
`connectors:*`, `inputs:` and `outputs:` properties supply the facts. The authored
`lunco:synthesizer` selector may choose a registered synthesizer, and the existing
`synth.<name>` hook seam lets a Rhai policy return the generated Modelica source plus
its unit merge and diagram layout. Rust still reads and validates the composed USD
graph; it does not need a new branch when a policy changes the dynamic building
behaviour. A different physical domain must still register its own typed synthesizer
contract rather than silently changing this network's boundary.

At runtime `lunco-usd-sim` asks OpenUSD to compute the collection's included prims, then
projects every included Modelica program facet into one generated composite Modelica
root. Acausal facets contribute `connect()` equations; causal-only blocks participate
through their `inputs:`/`outputs:` connections. The built-in policy's connected units
preserve independent equation subgraphs and route their public inputs/outputs through
the root. A hook-backed policy can replace that merge and the visual placements while
remaining constrained to the same composed members and public boundary. The generated
source exists only at runtime; USD remains the authored source of assembly truth and
Modelica remains the equation language.

```usd
def Xform "Battery" (
    prepend references = @lunco://components/power/battery.usda@</Battery>
) {}

def Xform "Motor_FL" (
    prepend references = @lunco://components/mobility/motor.usda@</Motor>
)
{
    float inputs:demand.connect = </Rover/Electrical.inputs:drive_left>
    custom token connectors:p.connect = </Rover/Battery.connectors:p>
}

def Scope "Electrical" (
    prepend apiSchemas = ["CollectionAPI:components"]
)
{
    float inputs:drive_left
    float outputs:soc.connect = </Rover/Battery.outputs:soc_out>

    uniform token collection:components:expansionRule = "explicitOnly"
    prepend rel collection:components:includes = [
        </Rover/Battery>,
        </Rover/Motor_FL>,
    ]
}
```

Acausal inside the generated DAE; causal at the Scope boundary, where cosim crosses to
physics, environment, controls, and telemetry. The actual part prims remain where the
vehicle assembly needs them; the collection groups them without duplicating them below a
network proxy hierarchy. Independent units share the root's stable path namespace, so
generated instances remain unique even when the same component appears more than once.

The projector rejects a connector targeting a component outside the collection, but a
scope containing multiple disconnected units is valid: the projector owns that graph
partition and emits a composite model. Electrical reference checks are evaluated per
generated unit, so two independent buses do not get falsely diagnosed as one
over-determined bus. USD multi-target connections remain multi-way Modelica `connect()` equations;
the projector never selects only the first target. Causal scalar properties instead
require at most one source (and network outputs exactly one); multi-source authoring is
an error rather than an implicit first-source choice.

The Modelica class is resolved from the loaded `.mo` source (`within` plus its declared
class). `info:sourceAsset:subIdentifier` selects a definition when a source contains
several; no class name is guessed from an asset path. A source that is still loading
keeps the network pending, and a source that fails becomes a terminal projection error.

The transient generated document is also a standard Modelica visual document. The root
class carries an `Icon` and a `Diagram`, and each generated synthesis unit is a placed
child instance with its own `Icon`/`Diagram`; the unit class contains the placed member
instances that its equations actually execute. The workbench reads these annotations and
the `connect()` equations from the same generated AST, so opening the root shows the
runtime unit topology and drilling into a unit shows its real member topology. This is
inspection of the executable projection, not a second visual-only network.

For the shipped electrical policy, the unit diagram also includes a labelled
power-bus rail. Every generated `connect(...)` carries a standard Modelica
`Line` route through that rail, including the horizontal icon stubs and rail
crossing, so a panel-to-load edge is visibly a branch of the common bus rather
than a direct solar-to-motor wire. The policy derives the visual hub from graph
incidence and lays the remaining members into deterministic branch lanes; no
component class is special-cased. Member coordinates are local to each owning
unit diagram, while unit coordinates belong to the generated root diagram. The
diagram extent is derived from actual member positions, which keeps larger
twins such as the eight-motor Summer Space School rover legible. These are Rhai
presentation decisions, not Rust-owned electrical knowledge.

The same `flow Real i` supplies the live energy-flow cue in the diagram. The
standard Modelica connector resolver records `Pin.i`, and the existing canvas
edge renderer reads the corresponding live node-state values to animate
directional dots along the authored `connect(...)` route. This is the same
generic mechanism used by the rocket/lander `FluidPort` diagrams; no electrical
special case or second visual graph is needed. A zero current intentionally
looks idle, and a missing flow state is an explicit diagnostic rather than an
invented animation.

The generated document has normal workbench provenance but no authored lifetime:
its `generated/` origin is classified from the document registry, not from a
copied UI list. The browser shows the root boundary separately from promoted
member telemetry and can expand each unit to the composed member path, source
asset, and Modelica class. Source-root loading is asynchronous, so a cold LunCo
library displays an explicit loading diagnostic and reprojects when the shared
engine announces completion; an unknown class is an explicit error card. When
the USD network disappears, its generated document and metadata are retired,
while authored `.mo` documents remain open.

The Rhai result must explicitly return `member_output_aliases`, even when the
policy promotes no member outputs. It chooses which declared member outputs
become root telemetry and what aliases they use. The result must also include
`units`, both layout sections, and `source_roots`; Rust validates those fields
and the returned AST against the composed facts, but does not emit or infer the
visual schema. `source_roots` is dependency metadata for generated classes that
are not USD members; standard Modelica root-segment discovery remains the class
resolver's source of truth, and the shared engine loads roots asynchronously
before the canvas resolves their authored icons and ports.

Each returned unit may set its `instance` independently of its generated class
`name`. The facts provide the deterministic default, while the policy owns any
custom naming; Rust only validates that instances are valid, unique, and do not
collide with the generated root interface before using those exact names for
runtime signal provenance.

## 2a. Authoring a device model: the four rules that are not obvious

Every rule below was learned by breaking it, and each break was silent — a compiled,
stepping, plausible-looking island that was wrong. They are stated as rules because none
of them is inferable from the Modelica language reference alone.

### One pin, no `Ground` — and exactly one potential-setting device per island

MSL's `Modelica.Electrical.Analog` builds devices on `OnePort` (two pins, `v = p.v − n.v`)
and requires a `Ground` to pin the reference node. LunCo's devices carry **one** `Pin` and
no ground, because the vehicle bus *is* the reference: every device hangs off the same
node, and the chassis return is not modelled. That is a deliberate deviation, and it moves
one obligation onto the author.

With no `Ground`, **`v` at the node is set by whichever device states it.** `Battery` is
that device:

```modelica
p.v = voltage_nom * (0.8 + 0.2 * soc) + p.i * R_internal;   // sets the node potential
```

Every other device must *read* `p.v` and state its **current**, never its voltage. Two
voltage-setting devices on one island over-determine the node; zero leaves `v` free and
the DAE structurally singular. Neither is reported as such — the first shows up as a
solver failure with an unrelated-looking message, the second as an island that never
publishes.

### A source states CURRENT, not POWER

The bug that cost the most. Both the panel and the motor were originally written as
constant-power devices:

```modelica
p.i = -power_rating / p.v;      // ✗ nonlinear in the unknown
```

`p.v` is an unknown of the same algebraic system, so `p.i · p.v = const` closes a
**nonlinear** loop across every device on the bus. The live backend pairs each algebraic
row with a variable and secant-solves it, and it cannot invert that pairing:

```
algebraic refresh row 2 cannot be solved for `…Battery.p.i`:
the residual does not depend on it
```

Written as current sources, the same island is linear in its unknowns and steps:

```modelica
p.i = -(area * efficiency * irradiance * max(0.0, cos_incidence)) / v_mp;   // panel
p.i = (rated_power / v_rated) * abs(demand) / max(0.01, efficiency);        // motor
```

This is also the physically honest direction: a PV module is a **photocurrent** source, and
a motor drive regulates **current** (torque ∝ current) while the bus voltage sets how fast
that current can be pushed. Divide by a **parameter** (`v_mp`, `v_rated` — the nameplate),
never by the solved node voltage.

⚠ **A constant-power device hides while the vehicle is parked.** At `demand = 0` the motor's
`p.i` is zero, the division collapses, and the bus solves. The fault appears the moment
current flows. A test that only asserts an island *steps* will not see it — which is why
`scenarios/tests/solar_domain_nested_ref.rhai` drives its rovers and asserts a non-zero
motor draw, rather than reading ports at rest.

### `output Real`, or nothing can leave a synthesized unit

The domain projection exposes a component quantity only when the Scope authors a
boundary `outputs:<name>.connect` to that member output. A plain `Real` is computed
inside a child unit and is observable only in the generated-source diagnostic/API view;
it is not a runtime port. Marking a reported quantity `output` is **not** a causality
claim about the circuit: `p.v` and `p.i` are still solved acausally by the connection
set. It says that the quantity may be projected through an authored Scope boundary.

Runtime callers read the stable boundary name (`get(thermal, "motor_temp_left")`,
`ReadPorts` name `soc`). They do not address generated child instances or their
internal member paths. The generated source/API uses a readable, deterministic spelling
for those internal instances; this is diagnostic metadata rather than another write or
wiring surface:

| member path suffix | in generated-source instance name |
|---|---|
| one segment below the network | that segment, Modelica-escaped |
| two or more segments | escaped parent + `__` + escaped leaf |
| same readable suffix used twice | projection error; the author must disambiguate the USD member paths |

So `/Rover/RockerL/Motor_FL` is emitted as `RockerL__Motor_FL`. The full USD path remains
the authoritative identity in the generated-source mapping; the shorter spelling keeps
icons, equations, and diagnostics readable without introducing a collision fallback.

The workbench exposes this generated source as a read-only Modelica document,
not as a poster. Its root diagram shows generated units, and drilling into a
unit shows the native LunCo members and their authored icons. The class cache
loads a bundled package root through `lunco_assets::models::package_files` and
the shared `ModelicaEngine`; this keeps LunCo visual resolution on the same
source/AST path as every other Modelica class without making the generated
policy or UI depend on MSL.

The network's own authored `outputs:soc` is the runtime contract — `get(elec, "soc")`
reads the value forwarded from the child unit.

### 2b. Prove a photovoltaic source reaches the battery

For a fixed rover panel, the minimum end-to-end acceptance is:

1. the `Electrical` scope compiles from the explicit battery/panel/load collection;
2. the panel publishes a positive authored boundary output such as `solar_power` under
   a lit environment;
3. the same root publishes `solar_incidence` and `soc`; and
4. the battery reports charging current, or its state of charge changes during a
   parked observation.

The mesh and the presence of a `SolarPanel` entity are not enough. A useful live
`ReadPorts` filter addresses only the Scope boundary:

```bash
curl -sS -X POST http://127.0.0.1:4101/api/commands \
  -H 'Content-Type: application/json' \
  -d '{"type":"ExecuteCommand","command":"ReadPorts","params":{"api_id":<electrical-api-id>}}' \
  | jq '.data.ports[] | select(.name == "solar_power" or .name == "solar_incidence" or .name == "soc" or (.name | test("SolarPanel\\.(power_out|generated_current_a)")))'
```

The API id comes from `ListEntities` for the composed `…/Rover/Electrical`
entity; it is not the Rhai entity id. A positive panel output together with a
matching battery current proves the source is electrically connected rather
than merely compiled. For a fixed +Y panel, test generation under a known sun
vector; do not require a tracker yaw response from a component that has no joint.

## 3. Why library loading looks the way it does

`assets/models/LunCo/` is a standard structured Modelica package — `package.mo` +
`package.order` + members declaring `within LunCo.Electrical;`. **Why standard-conformant
rather than a bespoke bundle?** So the rumoca CLI (`--source-root`, `MODELICAPATH`), the
workbench editor, and the runtime all resolve it by the same Modelica rules, and so a
future OpenModelica/SystemModeler can read it unchanged. Leaning on the language standard
is cheaper and more durable than inventing a loader.

The trap that made this a real bug: a USD program compiles through `cosim.rs` →
`Compile { extra_sources: [] }`, a path that seats **no** library search path. So
`import LunCo.Electrical` resolves via the CLI but not, without help, at sim time.
The application now builds one generic inventory of structured package roots under
`assets/models/` and uses the root segment of the unresolved qualified name as the
Modelica search-path key. `LunCo` is therefore ordinary package data; there is no
library-specific installer or root-name branch.

- **Bundled packages load demand-driven in the compiler.** On an unresolved
  `ER002`/`ER003`, the compiler seats the discovered structured package roots through
  the same source-root path used by other libraries, then retries. MSL remains a
  separate, larger demand-driven root and is installed only if the reference remains
  unresolved. This preserves lazy startup without making a particular library a Rust
  special case.
- **The editor uses the same root-segment rule.** A cold qualified class requests its
  package root from the shared engine asynchronously; the canvas shows Loading and
  reprojects when that root is ready. Generated `source_roots` are dependency metadata
  and can prewarm roots, but class discovery does not depend on a generated document
  or on the string `LunCo`.
- **A twin's own `.mo`** (`<twin>/models`) loads via `source_roots::load_twin_source_roots`,
  a `lunco-modelica` system watching `TwinRoots`; on mount it sends `LoadSourceRoot { Disk }`
  (rumoca's `load_source_root_tolerant`). **Why in `lunco-modelica`, not at the USD twin-mount
  site?** Because `lunco-usd` has no dependency on `lunco-modelica` and should not gain one
  just to poke the worker; the crate that *owns* the Modelica worker is the right owner of
  "load a twin's Modelica," and it already sees `TwinRoots` through the shared `lunco-assets`
  dependency.

**Gotcha worth its own line:** `lunco_assets::models::model_files()` is top-level only
(`MODELS_DIR.files()`), so a package under a subdirectory is embedded but invisible to it.
Use `package_files_live(pkg)` in the native runtime, which prefers the editable
filesystem tree and recurses; `package_files(pkg)` is the embedded/portable
snapshot API. This is exactly the bug that made the runtime blind to
`LunCo/Electrical/*.mo` even though `include_dir!` had baked them in.
