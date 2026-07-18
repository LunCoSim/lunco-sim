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

## 2. Why a physical network is ONE model, and acausal

The tempting design is: each component (battery, panel, motor) is its own program with
causal ports (`current_out → current_in`), wired together in USD. **It is wrong, and the
symptom tells you why.** With causal signal wires, *something* has to sum the four motors'
draw before the battery sees it — so you reach for a bus model with `n_loads`, an array of
currents, an index per wheel. Every one of those is a workaround for the wires being
directional. Add a fifth wheel and you must update a count *and* an array slot *and* an
order: three places that can disagree.

A circuit is not directional. It is acausal, and Modelica exists precisely to express
that: a `Pin` with a `flow` variable, connected with `connect()`, makes the tool write
Kirchhoff's current law itself.

```modelica
connector Pin
  Real v;            // shared at a node — every connected pin sees one voltage
  flow Real i;       // summed to zero at every node — Kirchhoff, written by the tool
end Pin;
```

So the electrical domain is **one model per vehicle**
(`vessels/rovers/<rover>/<rover>_electrical.mo`) that imports the `LunCo.Electrical`
component classes and `connect()`s them to a shared node. Add a fifth wheel → one
component and one `connect` line; the node equation grows on its own. No count, no array,
no summation code — because those were never the problem, only symptoms of modelling a
circuit as signals.

**Why not many small programs, then?** Because the cosim boundary is *causal* by
construction (`SimConnection`: an output drives an input, stepped sequentially). Kirchhoff
needs the node solved *simultaneously* — you cannot sum flows across prims that step
independently. Acausal therefore forces one model per network. That is not a limitation to
work around; it is the correct granularity.

In USD this is **one** `LunCoProgram` under a domain scope (doc 38's shape):

```usd
def Scope "Electrical" {
    def LunCoProgram "System" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset lunco:program:sourceAsset = @vessels/rovers/rucheyok/rucheyok_electrical.mo@
        float inputs:battery_capacity = 312.0   # the circuit's parameters, valued here
        float inputs:irradiance                 # boundary — cosim wires it to the sun
        float inputs:omega_fl                   # boundary — cosim wires it to a wheel
        float outputs:soc
    }
}
```

Acausal *inside* the bus; causal *at the boundary*, where cosim legitimately crosses to
physics (a motor's shaft speed comes from Avian's step) and environment (irradiance from
the sun). Synthesising that circuit `.mo` from the USD electrical graph, instead of
hand-authoring it, is doc 37's netlist-from-USD — the next step, deliberately not built
here so the acausal core lands first and provably.

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
