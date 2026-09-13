# Control programs, OBC/FSW, and live USD changes

> Status: Active · Audience: contributors to vehicles, control, USD projection, and Rhai

## Boundary

The vehicle is not a special-case program host. It is a USD-composed entity with a
generic command surface:

```text
avatar / network / Rhai / Modelica controller
                    │
                    ▼
          named input ports + authority
                    │  USD connections / port propagation
                    ▼
             vehicle actuator ports
```

The avatar is allowed to run the ports authored by the vehicle's `Controls`
mapping. The mapping translates shared `UserIntent` values into named ports; it
does not know whether the target is a rover, lander, spacecraft, or a future
vehicle. A vehicle with no `Controls` mapping is still a valid USD entity, but
the avatar has no control path to it. API and program writers can still use the
generic port surface when they have authority.

The three ownership rules are strict:

| Concern | Owner |
| --- | --- |
| Scene identity, physical topology, control mapping, ports and connections | USD |
| Continuous equations, state and control-law math | Modelica or a Rust mechanism that owns that hot path |
| Sequencing, guards, retries, mission policy and event reactions | Rhai task policy |

Rhai is orchestration. It may select a program, set a mode, arm/disarm, set a
route, or react to an event. It must not write throttle/steer/force every tick.
The task and Modelica paths use the same named-port substrate as the avatar;
they do not get a second vehicle-specific actuator API.

## OBC and FSW are compositions

OBC/FSW is a role made by composing ordinary `LunCoProgramAPI` children onto a
vehicle, not a Rust `OBC` component and not a special USD schema. A typical
vehicle can therefore be assembled as:

```text
Vessel
├── Controls              (intent → input-port mapping)
├── OBC                   (optional program scope / namespace)
│   ├── Mission           (.rhai: sequencing and mission policy)
│   ├── Safety            (.rhai: inhibit / safe-state policy)
│   ├── Guidance          (.mo or Rust program: continuous guidance law)
│   └── ControlAdapter     (named ports and USD connections)
└── physical actuator ports
```

The names are examples, not reserved paths. The runtime discovers a child by
`LunCoProgramAPI`; `info:implementationSource` selects its one source arm
(`info:sourceAsset`, `info:sourceCode`, or `info:id`). It never asks whether the child
is called `Mission`. A simple rover
may use one Rhai task program that writes `throttle`, `steer`, and `brake`. A more
complete vehicle may have a task program write a mode/goal into Guidance, with Guidance
producing the final actuator ports. A lander uses the same pattern with
`external_throttle`, attitude, force and torque ports.

Several Rhai programs are a program-library concern: each child has a stable
USD path and its own source, inputs, and policy. Programs may be nested below a
namespace such as `OBC`; discovery follows the composed USD hierarchy and
projects each program onto its immediate owner. Multiple independent programs
must use distinct named ports or an authored arbiter; the runtime never invents
a priority or last-writer-wins order.

## Routes and behavior trees

Route geometry is USD and route sequencing is a scene-level Rhai program. A
program reads its authored `inputs:subject` relationship and exact composed
route-point paths; the subject does not own the route. A missing point or
subject is unresolved and causes a visible safe stop, never an origin guess or
stale vessel-owned route.

The editor authors route points and the program through the normal document
journal boundary. It does not generate a second behavior format or copy route
data into a vehicle component. Existing policy remains the authority when
points are appended or reordered.

No mission or route is a normal state:

- no mission program → no task is projected and no autonomous policy runs;
- no route → the avatar still works if `Controls` exists;
- a program with no route subject holds by default and reports the missing
  authored relationship;
- adding the first route point authors it under a scene-level route scope;
- deleting the program removes the task policy but does not remove the vehicle or
  its avatar control mapping.

## Live USD rebuild policy

An authored intent is lowered to typed USD operations, journalled as one change
set, then projected from the composed stage. The editor never installs ECS
behavior state as a shortcut.

```text
ApplyUsdOps / AttachProgram
  ├─ transform-only edit        → update the live entity in place
  ├─ program source/metadata    → update the owning program projection in place
  ├─ program ports/connections  → author atomically, refresh dependents
  ├─ relationship/connection    → author incrementally, refresh dependents
  └─ composition / physics API  → rebuild the smallest affected physical scope
```

A full scene rebuild is reserved for changes whose composed meaning is
non-local—variant/payload composition or a physical schema change that changes
the ECS component set. Active-state edits use the same generic structural
reconciler to despawn or respawn only the affected subtree, so deactivating a
referenced route point does not reset an unrelated vessel. A full rebuild is
never the response to editing a task program, adding a route point, dragging a
pin, or changing a program string. The stage is the source of truth; the ECS is
disposable projection state.

The important lifecycle rule is a fixed projection boundary: all operations in
one user intent are applied before the live consumer reconciles them. This
prevents a program from appearing without its authored port contract, a program
from being read before its schema exists, or a route source from being projected onto a vessel
that is simultaneously being rebuilt.

## Runtime lifecycle

The task host owns execution state separately from the tree cursor. Reusable
composites reset after returning `Success`; a one-way mission must not be
re-entered merely because the composite reset. Replacing a program explicitly
resets the host to `Running`; a completed or failed program latches a safe hold
until an explicit re-arm or compatible route update. A pure append resumes at
the old point, while a reorder/delete/edit is a deliberate replacement and starts
from the new authored policy.

This is the same rule for every vessel type. Vehicle-specific behavior belongs
in authored ports, USD connections, Modelica equations, or registered Rust
mechanisms—not in a branch in the route editor or task host.
