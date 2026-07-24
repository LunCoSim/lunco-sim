# 54 — The Electrical Domain, and Modelica Libraries from USD

> Status: Active · Audience: contributors on cosim, USD assets, and the Modelica library
> Builds on: [20 — Modelica domain](20-domain-modelica.md), [22 — cosim](22-domain-cosim.md),
> [37 — model synthesis](37-model-synthesis-and-multidomain-composition.md),
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
the projector never selects only the first target.

When the `.mo` contains one conventionally named class, its package-qualified class is
derived from the path. When a source contains several definitions, author the standard
`info:sourceAsset:subIdentifier` property on the program facet; it is the authoritative
Modelica class name.

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
