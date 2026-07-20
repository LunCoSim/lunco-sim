---
name: visualize-physics-with-shaders
description: >
  How to make a simulated value VISIBLE in LunCoSim — a strut that reddens as it
  takes load, a battery that dims as it drains, a tyre that glows where it slips,
  a tank whose colour tracks temperature. USE THIS SKILL whenever the user asks,
  in plain words, things like: "make the legs glow red when they take the load",
  "show the heat / stress / charge on the model", "colour it by how hard it's
  working", "visualize the forces", "why is the colour animated instead of real",
  or "make the visuals follow the physics". (For the agent mid-code: a WGSL
  `struct Material`, `inputs:<name>.connect` on a bound gprim, `ShaderLook::live`,
  `SHADER_PARAM_BACKEND`, a uniform that stays at its default, or a colour ramp
  someone is tempted to write in rhai.) Project-specific and non-obvious: a
  visual is a CONSEQUENCE of physics and is wired, never scripted; the parameter
  must be declared in the shader's `Material` struct or the wire is refused;
  names are snake_case; and normalisation belongs in the MODEL, not the shader.
  For the physics itself use compose-multidomain-twin; for scene authoring use
  build-usd-scene.
---

# Visualizing physics values with shaders

**A visual is a consequence of physics, not a performance of it.** A strut turns
red because it is carrying load, on the same tick and by the same number the
solver computed. Nothing samples a clock, nothing tweens, and nothing in rhai
paints anything.

The chain is three links, each owned by the layer that knows the fact:

```
Modelica / avian  ──►  a port  ──►  a USD connection  ──►  a WGSL uniform
   computes it        publishes it     wires it            draws it
```

## The mechanism

There is no shader-specific plumbing. Shader parameters are ordinary **port
sinks**: `rewire_usd_connections` turns any `inputs:*.connect` into a
`SimConnection` without caring what the target is, `propagate_connections` writes
it through `PortRegistry`, and `lunco-render-bevy`'s `SHADER_PARAM_BACKEND`
receives it into `ShaderLook::live`, which `rebind_changed_shader_look` drains to
the GPU. Same graph as a thruster force or a battery load.

## Recipe

**1. Declare the parameter in the WGSL.** The engine reflects the `Material`
struct — field names, offsets, and the `//!@` annotations — straight out of the
file. A field that isn't declared here does not exist.

```wgsl
//!@ui      base_color  color "Strut colour"
//!@default base_color  0.55,0.57,0.60
//!@ui      load_frac   0 1   "Load fraction (driven)"
//!@default load_frac   0.0
struct Material {
    base_color: vec4<f32>,
    load_frac:  f32,
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The RAMP is a look decision and belongs here. The NUMBER being ramped is a
    // physics result and does not.
    let hot = vec3<f32>(1.0, 0.15, 0.05);
    return vec4<f32>(mix(material.base_color.rgb, hot, material.load_frac), 1.0);
}
```

**2. Publish the value as a model output.** Normalise where the rating lives —
the spring knows what it is rated for; the shader does not.

```modelica
parameter Real load_rated = 1500.0 "Load (N) the strut is rated for";
output Real load_frac "load / load_rated, clamped to 0..1";
equation
  load_frac = min(1.0, max(0.0, load / load_rated));
```

**3. Wire it on the BOUND GEOMETRY.**

```usda
def Mesh "LegPX_Strut" (prepend apiSchemas = ["MaterialBindingAPI"])
{
    rel material:binding = </Looks/StrutMat>
    float inputs:load_frac.connect = </DescentLander/LegPX.outputs:load_frac>
}
```

**4. Verify.** `read_ports` on the prim lists every declared parameter with its
live value; the inspector shows driven rows greyed out. If the value moves in
`read_ports` and the colour doesn't, the problem is the shader; if it doesn't
move there, the problem is the wire.

## Why the wire goes on the gprim, and what it costs

A `UsdShade` input belongs to the **material**, which is shared by every prim
bound to it. A driven value is the opposite — per-instance; four legs each report
their own load. So the bound geometry is where the meaning lives, and the engine
makes that prim's material private (`ShaderLook::unshared`) so one leg's glow
does not paint its three siblings.

**Be honest that this is a LunCo convention, not portable USD.** Attribute
connections are core Sdf, so this is spec-legal and round-trips — but `inputs:`
is a UsdShade convention, and connectability is gated by
`UsdShadeConnectableAPIBehavior`, which registers Shader/NodeGraph/Material and
never a Gprim. Hydra's `HdMaterialNetwork` never walks this edge; usdchecker's
shading validators never see it; Omniverse and MaterialX (`<geompropvalue>`)
ignore it. The `material:binding` chain is portable, **the drive is not**.

The standard answer to "one material, varying per gprim" is `primvars:` plus a
`UsdPrimvarReader` node — and it would delete `unshared` and the private material
outright. We don't use it yet because the binder resolves a single shader, not a
network, so a reader node would have nothing to evaluate it. That is a deliberate
deferral with a known migration path, not the absence of a standard.

## Gotchas

- **A name the `Material` struct does not declare is refused** — and that is the
  feature. It surfaces as a dangling-wire warning from propagation instead of the
  classic silent dead uniform. If your wire logs as dangling, check the WGSL field
  list before checking anything else.
- **Names are snake_case, because the reflection binds WGSL struct fields.**
  `inputs:loadFrac` and `inputs:load_frac` both reach `load_frac`, but a field
  spelled `loadFrac` in the WGSL is unreachable.
- **Publish the physical quantity, not the driving term.** `LegStrut.mo` once
  output the proximity-gated force pressed onto the leg, so a strut still 0.6 m
  in the air already read fully loaded and glowed red *before* touchdown. The
  honest output is the spring's own reaction (`k*x + c*v`), which is exactly zero
  until compression starts. **When a visualization "happens too early", suspect
  the model is publishing an input rather than a result.**
- **Normalise in the model, not the shader or a script.** `load_rated` lives with
  the spring that knows its own rating, so re-rating the strut re-scales the whole
  fleet's glow with no shader edit and no script.
- **Don't collide with an `//!@engine` field.** Fields annotated `@engine` are
  filled by Rust every frame (`sun_vis`, `albedo`, `sun_dir_world`,
  `weight_rough`). A wire pointed at one of those loses the race every frame,
  silently. Pick a name the engine does not own.
- **Driven parameters are read-only in the inspector.** Anything present in
  `ShaderLook::live` is engine-owned and its control is disabled — editing it
  would be a lie, since the next tick overwrites whatever was typed. Seeing the
  value move is the point; editing it is not.
- **`inputs:` is the spelling for EVERY port, not just shader parameters.** The
  same leg prim carries `inputs:altitude.connect` feeding its Modelica model. The
  material layer intersects against the shader's declared inputs precisely so
  those simulation wires are not mistaken for shader drives — if you add a
  parameter to the WGSL, check you have not just shadowed a sim port name.
- **A vec parameter cannot be driven by one wire.** A connection carries one
  `f64`. Drive components individually (`inputs:tint_r`) or ramp the colour inside
  the shader from a scalar — the latter is almost always what you want, because
  the ramp is a look decision.

## Anti-patterns

- ❌ Painting colour from rhai (`set(me, "PbrLook.emissive.red", …)`) — that is
  animation, not visualization; it re-derives in a script what physics already
  computed, and it drifts the moment the model is re-tuned.
- ❌ Ramping against a clock, a phase, or an altitude threshold instead of a
  physical result. If the shader needs to know the mission timeline, the value
  being drawn is the wrong one.
- ❌ Hardcoding the full-scale constant in the shader — re-rating the part then
  silently lies about every instance.
- ❌ Reading a SENSOR to drive a physical part's visual. A strut's glow follows
  its own reaction force, not an altimeter that sits 3.3 m away; see
  [`compose-multidomain-twin`](../compose-multidomain-twin/SKILL.md).
