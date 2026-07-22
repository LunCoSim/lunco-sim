# Lint substrate — facts in Rust, rules in policy

**Status: implemented.** Substrate `crates/lunco-lint`; USD facts
`crates/lunco-usd-avian/src/lint.rs`; rules `assets/scripting/policy/lint_usd.rhai`;
entry points `RunLint` (live scene) and `ValidateAsset` (file).

## What it is for

Some authoring mistakes have no symptom. In July 2026 every rover in the sandbox
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
| **Facts** | Rust, in the crate that owns the subject (`lunco_usd_avian::physics_facts`) | Only something holding the composed stage can answer "is this prim inside a body", "does any joint name it", "is there a collider in its subtree". Extracting that is code, and it is unit-tested as code |
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
facts.prims[]   #{ path, type, parent, schemas[] }     ← the GENERIC projection
```

`bodies`/`joints` are pre-chewed answers to the questions we already ask.
`prims` is the escape hatch that makes the rhai half real: a rule about a schema
nobody anticipated (`mass-outside-any-body` is the worked example) needs **no
Rust change**.

## Nothing lints on load

Linting is something you **run**, not something that runs at you. A check firing
on every scene load trains its reader to scroll past it and taxes play with an
opinion about authoring. So:

```rhai
cmd("RunLint", #{});             // lints every loaded stage
query("LintReport");             // { errors, warnings, findings[] }
```

…and the same verb over HTTP/MCP (`{"command":"RunLint"}`). A scenario that calls
both on a cadence **is** the realtime linter — no separate mode exists, because
none is needed.

Rules are hot-swappable at that same level:

```rhai
register_hook("lint.usd", "lint_usd", my_rules_source);   // next RunLint obeys
unregister_hook("lint.usd");                              // back to no USD rules
```

## Two entry points, one rule set

| | Subject | Reached by |
|---|---|---|
| `RunLint` | every **loaded** stage — including runtime spawns and edits no file describes | `cmd`/HTTP/MCP |
| `ValidateAsset` | one **file**, composed pre-flight | `sandbox --validate <path>`, HTTP query |

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
| `joint-target-not-a-body` | error | `physics:body0/1` names a prim that applies no `PhysicsRigidBodyAPI` — the joint is dropped at load and the mechanism is silently rigid |
| `dynamic-body-no-collider` | warn | a simulated, non-kinematic body with no collider in its subtree — it cannot touch the world |
| `mass-outside-any-body` | warn | `PhysicsMassAPI` on a prim that is not a body and sits inside none — the mass reaches no solver |

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
- `assets/scenes/tests/parts_attached.usda` — the **behavioural** counterpart:
  four rovers driven 12 s, and no descendant may move more than 0.5 m relative to
  its vessel. Lint catches the authoring; this catches the physics.

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
