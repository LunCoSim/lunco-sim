# Lint substrate — facts in Rust, rules in policy

> Status: Active · Audience: contributors adding lint facts or authoring lint rules

Substrate `crates/lunco-lint`; USD facts
`crates/lunco-usd-avian/src/lint.rs`; rules `assets/scripting/policy/lint_usd.rhai`;
entry points `RunLint` (live scene) and `ValidateAsset` (file).

## What it is for

Some authoring mistakes have no symptom. In July 2026 every rover in the luncosim
lost all four drive motors on the first physics step:
`components/mobility/motor.usda` applied `PhysicsRigidBodyAPI`, the motors were
children of the chassis body, and no joint named them — so each was a separate,
free, collider-less body that fell through the hull and was left on the regolith.
The rovers still drove, still steered, still hit their authored top speed. Every
parity gate stayed green. The bug was found in a screenshot.

That is the class this substrate exists for: **wrong in the authoring, invisible
in the simulation.** A runtime test can only catch it by simulating the exact
situation; a lint catches it by reading what was written.

## The split

| Layer | Where | Why there |
|---|---|---|
| **Facts** | Rust, in the crate that owns the subject (`lunco_usd_avian` for standard joints, `lunco_usd_sim` for gear drives) | Only something holding the composed stage can answer "is this prim inside a body", "does any joint name it", "is there a collider in its subtree". Each projection owner supplies the fields its runtime reader actually consumes, and the command layer composes them into one USD fact map |
| **Rules** | rhai policy, `assets/scripting/policy/lint_<domain>.rhai` | A rule that needs a rebuild is a rule nobody writes, tunes, or silences. These are editable against a **running** sim |
| **Findings** | `lunco_lint::LintReport` | One report, one shape, every domain |

`lunco-lint` is substrate: it knows what a finding is and how a domain asks
policy for one. It knows nothing about USD, rhai or Modelica.

## One linter per domain

Domains are separate because their subjects, vocabulary and audiences are
separate — one giant rule file is read by no one:

```
domain "usd"      → hook `lint.usd`      → assets/scripting/policy/lint_usd.rhai
domain "rhai"     → hook `lint.rhai`     → assets/scripting/policy/lint_rhai.rhai
domain "modelica" → hook `lint.modelica` → assets/scripting/policy/lint_modelica.rhai
```

A domain is just a name: `lunco_lint::run_lint(domain, facts)` invokes
`lint.<domain>` and parses the findings. **No policy registered ⇒ no findings**,
so an app built without scripting behaves exactly as before. Today the USD domain
is wired end to end; `rhai`/`modelica` facts come from `ValidateAsset`'s
per-extension pre-flight (`source`, `path`, parse errors) and grow from there.

### The policy contract

```rhai
fn lint_usd(facts) -> [ #{ rule, severity, subject, message }, … ]
```

`severity` is `"error" | "warn" | "info"`; anything else reads as `warn` — a typo
in a rule must not silently delete the finding it was written to raise. A policy
that faults or returns a non-array yields nothing and logs why: **a linter may
never break the thing it is diagnosing.**

### What the USD rules see

```
facts.bodies[]  #{ path, type, kinematic, simulated, collider, subtree_collider,
                   host_body, jointed }
facts.joints[]  #{ path, type, bodies[], missing[] }
facts.stage     #{ meters_per_unit_authored, fixed_hz, physics_substeps,
                   substep_dt }
facts.drives[]  #{ path, joint_type, body0, body1, realization,
                   stiffness, damping, max_force, driven_mass,
                   frequency, damping_ratio }
facts.gear_drives[] #{ path, valid, realization, ratio, rest_offset,
                       target_velocity, stiffness, damping, max_force }
facts.prims[]   #{ path, type, parent, schemas[] }     ← the GENERIC projection
facts.collision_enabled_without_api[]  paths authoring
                                          `physics:collisionEnabled=true` without
                                          `PhysicsCollisionAPI`; terrain and PhysX
                                          wheels are admitted by their owning
                                          projectors and are excluded
```

`bodies`/`joints` are pre-chewed answers to the questions we already ask.
`stage` carries the fixed-step contract from `lunco-core`/`lunco-physics`; rules
must read it rather than hardcoding a substep count. The engine-wide contract is
Avian's eight substeps per 60 Hz fixed tick. Prismatic drives use Avian's native
constraint solve; there is no target-specific second joint solve in the
production path.
`drives` is semantic output
from the same typed USD→Avian reader used by runtime projection, so the policy
does not duplicate USD unit conversion or motor-model selection. A
`spring_damper` realization is Avian's implicit, unconditionally stable model;
stiffness-bearing `force_based` and all `acceleration_based` spring realizations
are conditional models and are rejected by the shipped policy. A positive pure
ForceBased damper remains allowed because the owning USD→Avian mapping documents
that case as safe. `gear_drives` is semantic output from the
`lunco-usd-sim` reader for `PhysxPhysicsGearJoint`; its force and acceleration
realizations are implicit per substep, so positive coefficients have no guessed
asset-specific stiffness cap. Invalid values are retained and rejected by
policy before a run.
`prims` is the escape hatch that makes the rhai half real: a rule about a schema
nobody anticipated (`mass-outside-any-body` is the worked example) needs **no
Rust change**.

## Explicit authoring/preflight only

Linting is something you **run**, not something that runs at you. A check firing
on every scene load, every physics tick, or on a background cadence trains its
reader to scroll past it and taxes play with an opinion about authoring. So:

```rhai
cmd("RunLint", #{});             // lints every loaded stage
query("LintReport");             // { errors, warnings, findings[] }
```

…and the same verb over HTTP/MCP (`{"type":"ExecuteCommand","command":"RunLint"}`).
After an authored change, the editor or launcher may issue the command again for
that selected stage; it is still an explicit lint run. There is no cadence,
background watcher, per-tick physics monitor, or emergency clamp. `ValidateAsset`
does the same check for a file. Emergent contact/topology failures still require
the relevant behavioral test; a static lint must report "conditionally stable"
or "not certifiable" rather than claim a nonlinear assembled mechanism is safe.

Rules are hot-swappable at that same level:

```rhai
register_hook("lint.usd", "lint_usd", my_rules_source);   // next RunLint obeys
unregister_hook("lint.usd");                              // back to no USD rules
```

## Two entry points, one rule set

| | Subject | Reached by |
|---|---|---|
| `RunLint` | every **loaded** stage — including runtime spawns and edits no file describes | `cmd`/HTTP/MCP |
| `ValidateAsset` | one **file**, composed pre-flight | `luncosim --validate <path>`, HTTP query |

Both hand the policy the **same facts in the same shape**. `ValidateAsset` merges
the domain facts at top level for exactly that reason: nest them and `facts.bodies`
becomes `facts.subject.bodies`, every rule matches nothing, and a broken file gets
a clean bill of health. That happened once and is now pinned by a test.

`ValidateAsset`'s own per-extension checks are unchanged and are a different
tier: they are what the **loader** would refuse (parse, compose, `WheelParams`),
compiled because they are the loader's own code paths. Lint findings are what is
merely **wrong** — `error` severities join `errors`, everything else joins
`warnings`.

## The shipped USD rules

| Rule | Severity | Says |
|---|---|---|
| `nested-body-no-joint` | error | a body inside a body that no joint names — it will fall out of the vehicle. **The motor bug.** Exempt: disabled bodies, and `PhysxVehicleWheelAPI` wheels, which the drivetrain realizes (jointed in `physical`, raycast-driven in `raycast`) |
| `joint-target-not-a-body` | error | `physics:body0/1` names a prim that resolves to **no body at all** — the joint is dropped at load and the mechanism is silently rigid. Naming a non-body that sits *under* a body is fine and is how every mounted mechanism attaches (below) |
| `collision-enabled-without-api` | error | ordinary geometry authors `physics:collisionEnabled=true` without `PhysicsCollisionAPI`, so the USD-to-Avian reader ignores the intended solid shape. Terrain and PhysX wheels use their owning projectors instead |
| `connector-requires-network-root` | error | a `connectors:*.connect` **wire** authored outside every `CollectionAPI:components` scope — no compiler network owns it, so no `connect()` equation is generated and the pin solves as unconnected. A bare declaration is exempt (below) |
| `dynamic-body-no-collider` | warn | a simulated, non-kinematic body with no collider in its subtree — it cannot touch the world |
| `mass-outside-any-body` | warn | `PhysicsMassAPI` on a prim that is not a body and sits inside none — the mass reaches no solver |
| `conditionally-stable-joint-drive` | error | a stiffness-bearing USD drive resolves to Avian `ForceBased`, or any spring drive resolves to `AccelerationBased`, at the authoritative fixed step; use the implicit `SpringDamper` realization instead of tuning substeps to mask it |
| `joint-drive-negative-stiffness` | error | a drive has a negative stiffness coefficient that injects energy and cannot be converted to the implicit `SpringDamper` realization |
| `joint-drive-negative-damping` | error | a drive has negative damping (or an implicit drive would receive a negative damping ratio) and injects energy |
| `invalid-gear-drive` | error | a `PhysxPhysicsGearJoint` angular drive has values the canonical USD-sim reader refuses to install |
| `invalid-network-synthesizer` | error | the composed `CollectionAPI:components` members have incompatible domain roles and runtime cannot select an owner |

Network ownership is derived from the same composed role classifier used by the
runtime domain projection. A collection of `LunCoForceActuatorAPI` members is
owned by `actuator-wrench`; a collection of `LunCoProgramAPI` members is owned
by the generated Modelica synthesizer. The linter must not interpret an absent
selector as the default Modelica owner for a physical actuator collection. A
mixed or otherwise invalid role set is reported as `invalid-network-synthesizer`
instead of being assigned a fallback owner.

### Two rules that were measured firing on the whole fleet

Both were tightened after the shipped sweep reported them 387 times, and both
tightenings say the same thing: **a rule must fire on the claim, not on the
vocabulary the claim is made in.**

**A joint endpoint resolves upward.** UsdPhysics resolves an endpoint that is not
itself a body to its nearest ancestor body, and
`joint_endpoint_that_is_not_a_body_resolves_to_its_nearest_ancestor_body` in
`lunco-usd-avian` pins that the loader does it. That resolution is the whole
mounting mechanism for a *component*: `components/comms/antenna.usda` names its
own root `Xform` for `body0` because it is referenced onto rovers, landers, masts
and ground stations and knows none of their paths. Parenting **is** the
attachment. Reading "not itself a body" as the fault reported 73 antennas, dishes
and booms as broken mechanisms; the fact is now "resolves to no body", and
`missing` in the fact table means exactly that.

**A declared connector is an interface, not a wire.** `custom token connectors:p`
is the USD spelling of Modelica's `Pin p`, and a catalogue part cannot know
whether the vehicle composing it will wire that pin —
`components/mobility/motor.usda` declares its pin unconditionally, and
`rocker_bogie.usda` binds it to the selected power source in each `power` variant;
the default `power = "infinite"` realization uses an authored ideal source, while
the finite realization uses the stable battery assembly child. Reading the
declaration as the fault reported 314 errors, one per motor per rover, none of
them fixable without either wiring a source the scene deliberately omits or
stripping the pin from the part that owns it. The facts now carry `connectors`
**and** `connected`, and the rule reads the second.

The authoring rule underneath all of it: **hierarchy is namespace, a joint is
attachment.** An internal part is mass + geometry with no body; a part that must
move relative to its host gets a body **and** a joint, together — which is what
`AttachSpec` already authors for a mount. UsdPhysics says the same (a descendant
collider belongs to its ancestor body; a descendant body is a second body), and
so does every robotics dialect: URDF lumps a fixed-jointed link into its parent's
inertia, MJCF treats a jointless nested body as welded, and neither has a notion
of a link inside a link attached to nothing.

## What keeps it honest

- `crates/lunco-scene-commands/tests/shipped_assets_lint_clean.rs` — every
  shipped vessel/scene/mission/tutorial must be lint-clean, **and** the
  deliberately broken scene must still fail through the same path. "All clean"
  and "the rules never ran" are the same green square without that second test.
  `assets/components/` is deliberately out of scope: an overlay fragment
  (`physical_drivetrain.usda` is nothing but joints) cannot answer for
  joint targets that arrive with the reference arc. Components are covered
  through the vessels that compose them.
- `assets/scenes/tests/lint_selftest.usda` + `scenarios/tests/lint_selftest.rhai` —
  the chain end to end (facts → hook → rules → report → query), including the
  false-positive guard that a correctly jointed nested body stays silent.
- `assets/scenes/tests/parts_attached.usda` and
  `assets/scenes/tests/parts_attached_ackermann.usda` — the **behavioural**
  counterpart: each pair drives for 12 s, and no descendant may move more than
  0.5 m relative to its vessel. The pair fixtures keep physical warmup stable
  while covering both rover types and both drivetrain realizations. Lint catches
  the authoring; this catches the physics.

## Traps

- **rhai's expression-complexity cap.** A four-field map literal nested in two
  `for` loops fails to *compile*, which takes down the whole policy and leaves
  the linter registering nothing. Build findings field by field (the `finding()`
  helper) and messages with `+=`.
- **A rule with no runner is a comment.** Every rule shipped here is exercised by
  `lint_selftest.usda`; add a fault there when you add a rule.
- **Noise kills linters.** A rule earns its place only when the mistake is always
  wrong, invisible at runtime, and decidable from the authored stage alone.
  Anything that already fails loudly does not belong.
