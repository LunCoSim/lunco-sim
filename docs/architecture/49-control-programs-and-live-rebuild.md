# Control programs, OBC/FSW, and live USD changes

> Status: Active · Audience: contributors to vehicles, control, USD projection, BT and Rhai

## Boundary

The vehicle is not an autopilot special case. It is a USD-composed entity with a
generic command surface:

```text
avatar / network / Rhai / BT / Modelica controller
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
| Sequencing, guards, retries, mission policy and event reactions | BehaviorTree / Rhai policy |

Rhai is orchestration. It may select a program, set a mode, arm/disarm, set a
route, or react to an event. It must not write throttle/steer/force every tick.
The BT and Modelica paths use the same named-port substrate as the avatar; they
do not get a second rover-specific actuator API.

## OBC and FSW are compositions

OBC/FSW is a role made by composing ordinary `LunCoProgramAPI` children onto a
vehicle, not a Rust `OBC` component and not a special USD schema. A typical
vehicle can therefore be assembled as:

```text
Vessel
├── Controls              (intent → input-port mapping)
├── OBC                   (optional program scope / namespace)
│   ├── MissionBT         (.btxml: sequencing and mission policy)
│   ├── SafetyBT          (.btxml: inhibit / safe-state policy)
│   ├── Guidance          (.mo or Rust program: continuous guidance law)
│   └── ControlAdapter     (named ports and USD connections)
└── physical actuator ports
```

The names are examples, not reserved paths. The runtime discovers a child by
`LunCoProgramAPI`; `info:implementationSource` selects its one source arm
(`info:sourceAsset`, `info:sourceCode`, or `info:id`). It never asks whether the child
is called `Mission`. A simple rover
may use one BT program that writes `throttle`, `steer`, and `brake`. A more
complete rover may have a BT write a mode/goal into Guidance, with Guidance
producing the final actuator ports. A lander uses the same pattern with
`external_throttle`, attitude, force and torque ports.

Several BT programs are therefore a program-library concern: each child has a
stable USD path/id and its own source and ports. Programs may be nested below a
namespace such as `OBC`; discovery follows the composed USD hierarchy and
projects onto the owning vehicle. One BT.CPP asset may contain several named
trees, with `main_tree_to_execute` selecting one explicitly. Several independent
BT program children are currently fail-closed until an authored port arbiter
selects one; the runtime must never invent a priority or last-writer-wins order.
That arbiter is the next piece of the multi-controller catalog, separate from
loading and from the physical vehicle.

## Routes and behavior trees

Route geometry is USD. The BT XML is topology and policy. A `drive_to` leaf names
the waypoint prim; the compiler resolves that composed prim and bakes a runtime
position. A missing waypoint is an unresolved reference and retains the last
good compiled tree; it must never become the origin.

The editor's first waypoint creates a scene-root `Route` scope, a referenced
marker prim, a `LunCoProgramAPI` program when absent, and the source XML in one
`ApplyUsdOps` change set. The default shell is a one-way sequence. If an author
already chose a `forever`/repeat decorator, appending a waypoint preserves that
policy; the editor never silently changes mission semantics.

No mission or route is a normal state:

- no mission program → no BT is projected and no autopilot tree is engaged;
- no route → the avatar still works if `Controls` exists;
- an explicitly engaged autopilot with no tree holds by default, unless its
  caller explicitly requests constant cruise;
- adding the first waypoint authors the missing program and route through USD;
- deleting the program removes the BT policy but does not remove the vehicle or
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
non-local—variant/payload composition or a physical schema/active-state change
that changes the ECS component set. It is never the response to typing BT XML,
adding a waypoint, dragging a pin, or changing a program string. The stage is
the source of truth; the ECS is disposable projection state.

The important lifecycle rule is a fixed projection boundary: all operations in
one user intent are applied before the live consumer reconciles them. This
prevents a program from appearing without its authored port contract, a program
from being read before its schema exists, or a route source from being projected onto a vessel
that is simultaneously being rebuilt.

## Runtime lifecycle

The BT host owns execution state separately from the tree cursor. Reusable BT
composites reset after returning `Success`; a one-way mission must not be
re-entered merely because the composite reset. Replacing a program explicitly
resets the host to `Running`; a completed or failed program latches a safe hold
until an explicit re-arm or compatible route update. A pure append resumes at
the old leg, while a reorder/delete/edit is a deliberate replacement and starts
from the new authored policy.

This is the same rule for every vessel type. Vehicle-specific behavior belongs
in authored ports, USD connections, Modelica equations, or registered Rust
mechanisms—not in a branch in the waypoint editor or BT host.
