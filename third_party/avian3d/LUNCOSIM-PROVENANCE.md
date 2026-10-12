# Avian 3D source provenance

This is the published `avian3d` 0.7.0 crate (MIT OR Apache-2.0), from
upstream release `v0.7.0`, commit `965e85b`. Cargo uses this exact local source
through the root crates.io patch. The upstream licence files are retained.

The native prismatic and revolute motor owners integrate dimensional force and
acceleration drive coefficients with implicit Euler. They use the actual
coordinate inverse effective inertia that the native joint solver computes from
both endpoints and its current geometry. `ForceBased` stores N/m, N s/m (or the
angular equivalents); `AccelerationBased` stores their acceleration equivalents.
The existing native impulse application and force/torque limits remain the sole
application path. Pure damping and zero-gain motors use the same equation.

LunCoSim's USD reader therefore stores dimensional coefficients directly. It
does not convert them using authored or guessed body mass. Any future dependency
update must retain this dimensional integration contract and pass the production
drive, continuous-contact, landing-impact and rover-leg gates.

Native hard-geometry passes share the ordinary joint implementation, omit
compliant channels, and omit motor integration. Prismatic alignment and travel
limits use their respective compliance fields. Motor speed includes the anchor
angular contribution; its effective translational inertia respects locked axes.
Closed-form native motors do not warm-start a previous frame's motor impulse.
A zero force/torque cap clamps their admitted impulse to zero.

Net force/torque accumulation uses the impulse actually applied to body1.
Motor effort retains its positive body2 coordinate convention, so motors and
travel-stop reactions cancel correctly when they oppose each other.

3D XPBD velocity projection uses the shared `atan2` SO(3) logarithm, matching
native exponential-map position integration and the continuous trajectory.
It preserves small rotations and is invariant to quaternion norm drift.
XPBD position solving and velocity projection run after position integration
and before contact relaxation. Native friction therefore consumes the joint
and continuous-contact velocity corrections within the same substep.

Joint damping backports upstream PR 1078 (`de82c21fc1f7a34feec735b4444022bd394b2f4d`):
angular corrections use effective inverse inertia, free locked axes remain
available, and kinematic velocities are immutable. Its shared application
function lets LunCoSim consume the native math in its prepared stable order.
Completed joint reactions divide each substep multiplier by the actual native
substep duration, then average the accumulated impulse over the physics step.
Sleeping islands retain the last completed native reaction; disabled joints or
body endpoints clear it. Waking resumes native force writeback.

The native revolute coordinate uses signed `atan2` in its scalar precision.
Motors, co-simulation ports, mobility and the joint-state panel consume that
same coordinate; none narrows the canonical rotation to rendering precision.

Native contact manifolds retain Parry's composite subshape identities. The USD
adapter maps those identities through admitted collider provenance, so authored
Rhai gates can identify a contacting structural member within a compound body.

The native mass helper stops descendant reduction at nested rigid bodies.
Each body owns its own mass properties and its collider frame; a nested body's
mass and descendants cannot enter its parent's reduction. Traversal continues
through non-body grouping nodes and retains siblings after a nested boundary.
Collider insertion retains the constructor's scale and computes its initial
mass from that geometry. The native preparation system applies transform-driven
scale before mass reduction; render GlobalTransform does not rescale a newly
inserted collider.
Only live collider markers admit geometry-derived mass. Required mass-property
components retained after collider removal cannot contribute retired geometry;
independent explicit mass components still retain their authored contribution.

Native independent body queries use Bevy's batching strategy with the shared
`SolverConfig.minimum_body_batch_size` lower bound (default 64). Populations at or below
that bound execute the same body kernel directly on the caller; larger
populations retain native adaptive sizing. A conservative query population
upper bound chooses dispatch without visiting entities.
Velocity-increment preparation/clearing, integration, solver-body preparation,
angular-inertia refresh and writeback consume the same setting. Both standalone
body and integrator plugins initialize that resource without replacing an
installed configuration. This is a local scheduling change: body equations,
filters, precision, substeps and ordered contact/joint solving are unchanged.
