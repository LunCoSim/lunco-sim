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

## 2. USD assembles components; runtime projects one acausal model

Each physical component applies `LunCoProgramAPI` and names its reusable Modelica class.
Compiler-network members explicitly author
`info:implementationSource = "sourceAsset"` and a `.mo` `info:sourceAsset`; built-in
`info:id` programs and inline `info:sourceCode` programs are valid elsewhere, but are not
Modelica compiler inputs.
Its causal boundary uses `inputs:`/`outputs:`; its acausal Modelica connector members use
`connectors:`. Ordinary USD property connections author topology. There is no electrical
USD schema and no exposed `Pin` prim. An ordinary network `Scope` applies the standard
multiple-apply `CollectionAPI:components`; that collection is the explicit working set
for one projected Modelica model.

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

**Which synthesizer runs is authored.** `uniform token lunco:synthesizer` on the network
scope names one from the open `SynthesizerRegistry`
(`lunco_usd_sim::domain_projection`); absent means `"acausal-network"`, the built-in that
this section describes. A new physical domain — thermal, harness, comms-link — is a
`DomainSynthesizer` impl plus a `register()` call from any plugin: no enum, no edit to the
projector. (The synthesizer body is Rust today; moving the netlist-mapping POLICY into rhai
needs an emit surface that does not exist yet.)

At runtime `lunco-usd-sim` asks OpenUSD to compute the collection's included prims, then
projects every included Modelica program facet into one generated Modelica wrapper.
Acausal facets contribute `connect()` equations; causal-only blocks participate through
their `inputs:`/`outputs:` connections. The wrapper instantiates the qualified classes
and emits the equations. It exists
only at runtime; USD remains the authored source of assembly truth and Modelica remains
the equation language.

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
network proxy hierarchy. Separate electrical islands use separate Scopes and collections.
Their path namespaces give generated instances stable unique names even when the same
component appears more than once.

The projector rejects, and `lint.usd` reports, a scope containing multiple disconnected
acausal islands or a connector targeting a component outside the collection. This is
intentional failure isolation: one independently compiled network has one explicit USD
scope. USD multi-target connections remain multi-way Modelica `connect()` equations;
the projector never selects only the first target. Causal scalar properties instead
require at most one source (and network outputs exactly one); multi-source authoring is
an error rather than an implicit first-source choice.

When the `.mo` contains one conventionally named class, its package-qualified class is
derived from the path. When a source contains several definitions, author the standard
`info:sourceAsset:subIdentifier` property on the program facet; it is the authoritative
Modelica class name.

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

### `output Real`, or nothing can read it

The domain projection publishes a member's **`output`** variables as ports on the island.
A plain `Real` is computed every step and observable by nobody — no scenario, no HUD, no
telemetry channel. `DCMotor.electrical_power` was a plain `Real` for exactly this reason
and read back as an absent port, which is indistinguishable from an island that failed to
publish.

Marking a reported quantity `output` is **not** a causality claim about the circuit: `p.v`
and `p.i` are still solved acausally by the connection set. It says only that the number
leaves the model.

### Port names are the MANGLED prim path, and the mangling is total

A member's variable surfaces on the island as `<mangled member prim path>.<var>`. The
escaping (`instance_identifier` → `modelica_path_identifier`) is injective, not cosmetic:

| in the prim path | in the port name |
|---|---|
| `/` | `_x2f_` |
| `_` | `__` (doubled) |
| any other non-alphanumeric | `_x<hex>_` |

So `/Rover/RockerL/Motor_FL` reads as `Rover_x2f_RockerL_x2f_Motor__FL` — **two**
underscores in `Motor__FL`. Prims without an underscore in their name (`Battery`,
`SolarPanel`) hide this rule completely; every motor trips it. A misspelled port name
returns absent, which reads exactly like a broken island, so check the spelling before
believing the island.

The network's own authored `outputs:soc` is a *boundary* name and is not how a member's
interior variable is addressed — `get(elec, "soc")` reads nothing.

### 2b. Prove a photovoltaic source reaches the battery

For a fixed rover panel, the minimum end-to-end acceptance is:

1. the `Electrical` scope compiles from the explicit battery/panel/load collection;
2. the panel publishes a positive `power_out` under a lit environment;
3. the same island publishes `cos_incidence` and `Battery.soc_out`; and
4. the battery reports charging current, or its state of charge changes during a
   parked observation.

The mesh and the presence of a `SolarPanel` entity are not enough. A useful live
`ReadPorts` filter includes the scope boundaries and the member paths:

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
`Compile { extra_sources: [] }`, a path that seats **no** library. So `import
LunCo.Electrical` resolves via the CLI but not, without help, at sim time. Two mechanisms
close that, **both using rumoca built-ins** — the choice to reuse them rather than
hand-gather files is deliberate: the built-ins already do standard package parsing
(`package.mo`/`package.order`, `within` resolution), and reimplementing that is how bugs
like a non-recursive file scan creep in.

- **The shipped library loads demand-driven in the compiler.**
  `ModelicaCompiler::ensure_lunco_installed()` seats the embedded package (via
  `load_source_root_in_memory`) inside `compile_loaded`'s unresolved-reference retry.
  **Why in the compiler, not at startup?** Because that one location is on *both* the
  editor and cosim compile paths, so neither needs its own copy of the logic — and because
  it mirrors the existing demand-driven MSL gate exactly, so there is one install pattern,
  not two. **Why demand-driven and cheapest-first?** MSL is 316 MB; `LunCo` is a handful of
  embedded docs. Any unresolved reference earns the cheap `LunCo` install, but MSL is
  reached for only if refs are *still* unresolved afterward — otherwise every EPS model
  (which references `LunCo`, never MSL) would drag MSL in for nothing.
- **A twin's own `.mo`** (`<twin>/models`) loads via `source_roots::load_twin_source_roots`,
  a `lunco-modelica` system watching `TwinRoots`; on mount it sends `LoadSourceRoot { Disk }`
  (rumoca's `load_source_root_tolerant`). **Why in `lunco-modelica`, not at the USD twin-mount
  site?** Because `lunco-usd` has no dependency on `lunco-modelica` and should not gain one
  just to poke the worker; the crate that *owns* the Modelica worker is the right owner of
  "load a twin's Modelica," and it already sees `TwinRoots` through the shared `lunco-assets`
  dependency.

**Gotcha worth its own line:** `lunco_assets::models::model_files()` is top-level only
(`MODELS_DIR.files()`), so a package under a subdirectory is embedded but invisible to it.
Use `package_files(pkg)`, which recurses. This is exactly the bug that made the runtime
blind to `LunCo/Electrical/*.mo` even though `include_dir!` had baked them in.
