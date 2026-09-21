> Status: Design · Audience: LunCoSim geometry, SysML, USD, and Twin authors

# Parametric geometry and assembly constraints

LunCoSim should have one authored system model, one scriptable construction and
policy layer, and small reusable numerical kernels. Engineering intent must not
be duplicated between Rust constants, Rhai arrays, Modelica literals, and USD
metadata.

| Concern | Owner | Responsibility |
| --- | --- | --- |
| Design truth | SysML v2 source | Component tree, typed dimensions/units, datums, materials, requirements with rationale, and reusable constraint definitions |
| Construction and policy | Rhai source | Interpret the typed model, choose direct-placement or Modelica policy, make component geometry and a reviewed typed Editor plan |
| Language/model support | Rust core | Parse and resolve SysML into typed values/expressions/references; expose existing `f64` Bevy math types and reusable geometry algorithms to Rhai |
| Coupled solve | Modelica/Rumoca | Solve generated algebraic or dynamic equations when a closed-form Rhai policy is not appropriate; return typed results and solve evidence |
| Persistence/rendering | LunCoSim Editor and USD | Review and apply generation-fenced operations; store component hierarchy, transforms, `UsdGeomMesh`, looks, and applicable standard physics schemas |

The key distinction is authorship versus execution: Griffin geometry recipes,
dimension choices, constraint policies, and test scenarios are not authored as
Rust code. Rust supplies algorithms and bindings, not Griffin design rules or a
second design database.

## Geometry operations

The reusable numeric surface currently includes native Bevy `DVec2`, `DVec3`,
`DQuat`, and `DTransform`, bounds and oriented-box relations, convex-profile
extrusion, and profile revolution about local +Y. `revolve_profile` consumes a
closed polygon of typed `(radius, height)` `Vec2` values and returns a native
indexed `RevolvedProfileMesh`. It validates finite non-negative radii, profile
closure/topology, winding, and tessellation bounds; axis points become triangle
fans rather than degenerate rings. Both profile operations return ordinary
points, face counts, indices, and outward normals. Rhai authors dimensions and
policy; only the Editor adapter formats those values for a `UsdGeomMesh` op.

The kernel is deliberately not a B-rep or a general CAD solver. The existing
`LunCoLatheAPI` is a focused USD-side generator for its named bell and
paraboloid profiles; it is not a general-purpose source for vehicle dimensions.
For Griffin, the radius/profile is derived from the typed SysML contract and
the generated result uses standard `UsdGeomMesh`. Do not add a custom
`ParametricCAD` schema that duplicates SysML dimensions or implies a constraint
solver USD does not contain.

## Relationship authoring and solve

Use SysML v2 for typed component parameters, units, physical datums, requirement
rationale, and reusable relation definitions (`constraint def`). Define the
shared vocabulary in `LunCo.Geometry`: coincidence, fixed distance, segment
length, parallel/coaxial axes, point-on-plane, symmetry, containment, and
clearance.

Author concrete assembly relations and placement policies in Rhai, through a
small declarative embedded API—not as imperative per-part coordinate plumbing.
The shared `spatial_relations` tool now implements the first direct-placement
relations over native f64 `Vec3`, `Quat`, and `Transform` values:

```rhai
let leg_pose = spatial_relations::coincident(bus_mount_world, leg_mount_local);
let foot_station = spatial_relations::point_on_axis(strut_frame, local_axis, half_length);
```

The API also has fixed-frame offset placement, translation-only coincidence,
and translation/rotation residual measurements. The Griffin landing-leg Rhai
gate executes positive closure checks for rotated and translated coincidence,
fixed-frame offset, translation-only placement, and a dimensioned strut
station. The visual builder uses the same axis-station relation instead of
repeating its local sine/cosine endpoint formula.

The SysML projection now exposes source-snapshot-scoped element and feature
handles, relationship feature ends, and a source-spanned expression tree whose
feature leaves carry resolved handles. Rust's role is generic parsing,
resolution, and stable typed projection. Rhai owns the domain mapping from
formal features to bound datums, relation selection, DOF policy, and lowering
of the resolved expression tree into Modelica scalar equations. This keeps the
SysML expression—not a duplicate Rhai formula or Rust constant—as the equation
source of truth.

The first coupled policy accepts multiple `CoincidentPointTranslation`
usages for one moving owner and one fixed owner. It anchors three translation
unknowns from the first relation, lowers the authored expression to generated
Modelica equations, and emits Rumoca-evaluated residuals for every selected
relation. The Rhai report distinguishes the three anchor equations from the
additional closure conditions and includes the source handles and experiment
identity. This is a bounded translational relation policy, not a general mate
solver: rotational DOFs, arbitrary relation sets, rank-revealing analysis,
underdetermined families, and best-fit placement remain open.

Do not add a standalone CAD language or a custom USD schema. SysML v2 already
provides formal constraint definitions/expressions, and Rhai is the live
policy/tool layer. Keep the source-spanned expression projection generic and
typed; domain-specific feature mapping and Modelica lowering belong in Rhai.
If Rhai relation authoring later needs concise syntax, add sugar that lowers
to the same typed Rhai relation values—not another source of dimensions,
solver semantics, or identity.

Rhai owns direct and inspectable policies. For a relation with a fixed anchor,
it can use the already-exposed `DVec3`, `DQuat`, and `DTransform` operations to
compute placement and residual evidence. Do not add a Rust `PointConstraint`
API that merely wraps vector subtraction and length. For coupled,
underdetermined, or dynamic relations, a Rhai tool generates a small Modelica
model from the typed relation graph, submits it asynchronously, and reads back
typed values plus solve/source identity. A result is an input to a reviewed,
generation-fenced Editor placement plan, never an implicit USD write.

Modelica is not the geometry authoring language, and USD physics joints are not
design-time assembly mates. Use standard `UsdPhysicsJoint` schemas only where a
runtime physical articulation is actually intended. Store generated static
surfaces as standard `UsdGeomMesh`; do not invent `ParametricCAD` or imply that
USD itself can solve design constraints.

## Remaining gaps, in order

1. Add relation families as demanded by Griffin interfaces—axis alignment,
   fixed distance/length, symmetry, containment, and clearance—with positive
   Rhai evidence for measured residual and intended interface fit.
2. Extend the relation-set policy from translation to rotation-aware rigid
   transforms, with explicit frame contracts and residuals. Then add a genuine
   rank/DOF report for supported relation Jacobians; do not label relation
   counts as rank for arbitrary systems.
3. Add an optimizer policy only when Griffin needs a best-fit solution for
   noisy or overdetermined relations. The pinned Rumoca source has
   `rumoca-opt` for differentiable DAE parameter fitting (`RhsMseObjective`,
   forward/reverse gradients, and gradient descent), plus Jacobian and linear
   solve machinery. That crate is not in the current LunCoSim dependency/API
   surface, and its RHS-MSE training objective is not a geometric mate
   objective. Rumoca's pinned `rumoca-phase-structural` does provide
   incidence-based maximum matching and structural unmatched-equation /
   unmatched-unknown diagnostics; these identify generic structural
   under/over-determination, not geometric DOF or numerical Jacobian rank.
   Its pinned `rumoca-eval-solve` surfaces exact AD Jacobians for state
   derivatives and parameter sensitivities, plus a matrix-free VJP for
   implicit algebraic residuals; it does not currently give LunCoSim a
   direct general geometric residual-Jacobian/rank API. It is useful machinery
   to reuse, not a drop-in CAD optimizer. Any reuse should be exposed through
   a typed asynchronous API and preserve Rhai-authored relations, objectives,
   and solve evidence. See the [pinned structural analysis source](https://github.com/LunCoSim/rumoca/blob/e6884d035f700955e4e2cccaaa6230ca79f21e20/crates/rumoca-phase-structural/src/lib.rs),
   [Jacobian source](https://github.com/LunCoSim/rumoca/blob/e6884d035f700955e4e2cccaaa6230ca79f21e20/crates/rumoca-eval-solve/src/jacobian.rs),
   and [implicit sensitivity source](https://github.com/LunCoSim/rumoca/blob/e6884d035f700955e4e2cccaaa6230ca79f21e20/crates/rumoca-eval-solve/src/runtime/sensitivity.rs).
4. Add a typed, inspectable placement plan that consumes those relations and
   returns proposed transforms plus residual evidence. Keep Editor generation
   checks, review, and apply as separate steps.
5. Add a generic swept/lofted-section geometry kernel using `DTransform`
   section frames for struts, yokes, rails, and panel frames. Keep dimensions
   and cross-sections in SysML and geometry-construction policy in Rhai.
6. Add reusable positive mesh-integrity evidence (closed edge incidence,
   outward winding, finite normals, measured bounds, and volume) exposed to
   Rhai. Component tests should assert intended shape/profile/topology and
   interface fit, not obsolete implementation names.
7. Run the complete Griffin visual-builder path against the production Rhai
   tool set in the live Editor and inspect its projected geometry there. The
   current component gate validates relation arithmetic and composed geometry,
   but does not invoke the Editor builder.

All Griffin geometry/model acceptance checks belong in Rhai test assets at the
same public source-to-Editor boundary used by authors. Rust tests must not embed
Twin files, SysML fixtures, Griffin dimensions, or authored design expectations.

The user-facing Twin should record unresolved numerical/design facts as
assumptions with rationale and source links. A renderable mesh is not evidence
of material strength, collision ownership, or flight qualification.

## Standards references

- [OMG SysML v2 language specification](https://www.omg.org/spec/SysML/2.0/Language/PDF)
  defines constraint definitions/usages and formal requirement constraints.
- [Modelica language specification](https://specification.modelica.org/)
  defines acausal equation systems where variables are solved together.
- [OpenUSD `UsdGeomMesh`](https://openusd.org/release/api/class_usd_geom_mesh.html)
  defines indexed polygon geometry; [OpenUSD `UsdPhysics` joints](https://openusd.org/dev/api/usd_physics_page_front.html)
  define simulation-time rigid-body joints, not design-time CAD mates.
