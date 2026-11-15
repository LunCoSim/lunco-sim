# USD system structure and diagram review

Scope: the USD propulsion assembly, source-interface/type projection, generated
Modelica policy, composed Connections projection, view artifacts, Twin authoring
plans, and canonical authoring skills. Baselines: LunCoSim `21eeefa75` and
lunar-base-model `dc290a9`.

## Ownership and structure

- USD owns installed part identities, component references, collection membership,
  and tank/valve/chamber connections. The reusable engine has three explicit
  program parts and no nested solver collection.
- Modelica owns valve response, pressure/flow/enthalpy conservation, chamber
  performance, and thermal state. The enclosing propulsion collection retains
  one generated runtime participant and its existing public command/telemetry
  boundary.
- The existing Modelica source-interface owner supplies source-scoped declaration
  types. These are presentation facts, not a replacement for compiler type
  resolution. Inherited or wildcard-imported declarations can lack qualified
  direct-source facts; the diagram retains unclassified connectors in that case.
- Rhai owns assembly plans, domain grouping, naming, and generated diagram
  annotations. Rust owns composed facts, validated result application, source
  leases, and generic rendering/port geometry. No vehicle-specific Rust path,
  custom scene graph, or alternate USD writer was introduced.
- The complete Connections graph includes static scalar links, acausal links,
  and joints. Generated Modelica views cover their own equation networks.
  Saved views retain stable source URIs and exact graph identities.

## Findings addressed

1. Local boundary-input graphics must emit one bundle per receiving component.
   The regression scene gives one receiver two boundary inputs and requires
   one caption per input without duplicated bundles.
2. Rhai string replacement mutates its receiver. Port-caption construction must
   return the modified string explicitly; returning the replacement operation
   leaves empty generated captions. The same production regression checks the
   actual emitted captions and solved signal binding.
3. A feed-authoring plan must check every required valve, chamber, plume and wall
   before reporting success. The Twin's dry authoring gate omits one valve and
   requires a named rejection with an empty operation list, while the complete
   assembly retains its 39-member plan.
4. Twin startup instructions identify the required shared engine-assembly asset
   and minimum engine commit, including its publication branch. A compatible
   binary without that asset set does not satisfy the installed model contract.
5. The dated tether receipts describe their captured sources. Their guide now
   identifies the integrated native engine commit rather than claiming those
   changes are uncommitted.

## Verification

- `generated_boundary_graphics.usda` through the production scene-test binary:
  **5/5**, exit 0. Checks current source admission, readable unique captions,
  retained boundary inputs, and the solved receiver value.
- Mounted Griffin dry authoring checks: **3/3**, including the missing-valve
  rejection and the complete 39-member positive control.
- Prior unchanged-equation integration evidence remains applicable: engine-out
  **48/48**, starvation **11/11**, and full landing/tether egress **7/7**, each
  through production scene tests with exit 0. The egress gate completed at tick
  8806 with four fixed-support wheels, zero final line loads, and 0.10257 m of
  manual-drive proof.
- View acceptance: **28/28**, covering four reopened files, all 28 physical feed
  connections, preservation of 1732 exact graph edges through grouping,
  port inspection, negative grouping rejection, and ready/no-fault diagnostics.
- Canonical skill catalogue and local links are checked by
  `python3 scripts/validate_skills.py`.

## Model limits and handoff

Four visible tank stations are geometry; two reservoir models own propellant
mass. Pressure/feed values and RCS consumption retain their documented study
assumptions. The tether is a reduced bridle/winch study; its native wrench bridge
is scenario-owned and is not represented as fabricated static USD wiring.
Internal engine views intentionally omit external tank/control endpoints; the
feed and full-system views provide that context. Group prim counts include
geometry and must not be presented as numerical component counts.

The canonical skill guidance is
[Usable USD system models](../../skills/compose-multidomain-twin/references/inspectable-usd-systems.md).
It is shared by component authoring, vehicle building, interactive authoring,
Modelica generation, and UI acceptance so future models use one maintained
structure and usability contract.
