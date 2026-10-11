# Griffin contact and Avian integration review

The startup engine-off impact and powered rover-leg collision gates pass. This
review covers contact correctness, native drives, collider provenance and the
document operations needed to author that provenance. Real-time performance
acceptance remains open; passing these gates does not establish a tick budget.

## Authoritative owners

`lunco-physics` closes continuous material-contact inequalities and affected
native hard-joint geometry inside the same Avian substep, before velocity
projection. It consumes actual solved COM motion in f64, authored CCD settings,
native collision hooks, stable collider identities and native composite BVHs.
Changed trajectories are revalidated before writeback. Compliant channels and
motors retain their ordinary native integration; closure does not repeat them.
Initial contacts use native prepared geometry, positive-time impacts use valid
native cast witnesses, and mesh features use native normal constraints.
Unsupported geometry, ambiguous rotation increments and failed convergence
raise owner faults and refuse the affected tick's pose writeback.

Native support bounds include the entire rotational arc and changing support
features. Initial contact geometry is reused only while its starting geometry
is immutable in one substep. Parry's native GJK/EPA accepts the finite-feature
direction as its initial search direction for convex triangle pairs; it still
computes the depth. Finite edges, changing tilt, offset COM motion and changed
endpoints must retain their sweep when the full-arc bound cannot certify it.

The pinned Avian source owns dimensional implicit force/acceleration motors,
both-endpoint coordinate inertia, anchor velocity and force saturation. It also
owns SO(3) velocity projection, signed revolute coordinates, completed joint
reaction accumulation and joint damping. Sleeping islands retain their last
completed reaction; disabled joints or endpoints clear it. USD omitted drive
gains remain zero and omitted force caps remain unbounded. Readers, commands,
co-simulation and telemetry consume these owners rather than reconstructing a
law from one body's mass. Aliased endpoints fail before mutable body access.

USD owns geometry and provenance. The registered `LunCoDerivedGeometryAPI`
validates exact Cube/Cylinder proxies against their composed sibling source in
native double precision before collider admission. Admission publishes actual
source membership and native subshape identities. The exact rover collider
hierarchy, including wheels, supplies contact evidence. Authored Rhai chooses
the structural members and their body owners; Rust applies generic validation.
Griffin regenerates source geometry first, then its 40 landing-gear proxies
after the document generation composes.

The document owner removes attributes and relationships through local property
operations with `RestoreProperty` inverses. Undo preserves unrelated edits and
exact target list operations. Live projection consumes the document's resolved
property so removing a strong opinion reveals the weaker opinion correctly.

Hook review: continuous math, inertia, collision admission and safe writeback
are engine invariants with typed native inputs and outputs, installed for the
simulation lifecycle. They are required mechanisms rather than changeable
scenario policy. Member selection and source regeneration remain Twin Rhai;
lint selection and severity remain the installed USD Rhai policy.

## Maintained dependency assessment

[Avian 0.7.0](https://github.com/avianphysics/avian/releases/tag/v0.7.0) is the
latest published release checked on 2026-10-11 and matches Bevy 0.19. The local
patch retains its published source and licences, with exact changes documented
in `third_party/avian3d/LUNCOSIM-PROVENANCE.md`. Joint damping incorporates the
native correction from [PR 1078](https://github.com/avianphysics/avian/pull/1078).
Native body-only CCD does not establish articulated closure. The inspected
unpublished CCD work in [PR 1058](https://github.com/avianphysics/avian/pull/1058)
does not supply acceptance for this cylinder-based landing gear.

## Verification

Production binary: `cargo build -p lunco-luncosim -j 4`, repository target and
regular sccache. Authored gates used owned API port 4141, fixed epoch, 60 Hz,
one compute thread and zero jitter. Each gate returned exit code zero.

| Production gate | Result |
| --- | --- |
| `PRISMATIC_SPRING` | 24 checks; loaded equilibrium, momentum balance, sleeping reaction retention and disable clearing |
| `NATIVE_FORCE_DRIVES` | 18 checks; both-endpoint inertia, SI law, zero gains/caps, damping, stop reactions and unbounded caps |
| `CONTINUOUS_CONTACTS` | 12 checks; coupled contact, rotating fixed attachment, exact pair scope and initial-contact boundaries |
| `DERIVED_PRIMITIVE_COLLISION` | Expected `usd-avian-collider-invalid` fault after live source drift |
| `INVALID_JOINT_ENDPOINTS` | Expected `usd-physics-joint-invalid` fault after live endpoint aliasing |
| `LINT_SELFTEST` | 53 checks, including isolated invalid drive cases and valid controls |
| `PRISMATIC_DRIVE` | 5 checks under gravity |
| `GRIFFIN_ROVER_LEG_COLLISION` | 88 checks over 900 ticks; all 40 admitted structural sources and actual collision response |
| `GRIFFIN_ENGINE_OFF_IMPACT` | 9 checks over 1800 ticks; no thrust, pad centers/rims, stops and final four-pad support |

The final headless impact measured maximum native penetration 2.246854 mm,
minimum sampled pad clearance -2.353147 mm, hard-stop error 4.371647 nm and final
ground speed 1.493247 mm/s. The rover reached 2.454570 m/s and contacted the PX
shock-barrel tube collider, with maximum pair penetration 0.063022 mm and a
positive native collision impulse. No collision filters or authored mechanical
limits were weakened to obtain these verdicts.

Low-level tests cover contact trajectory bounds, SO(3) derivatives, finite mesh
features and native normal constraints, immutable contact-cache lifetime, local
property undo and resolved weak-property projection. The final changed geometry
kernel was checked with `ccd::tests::continuous_mesh_casts_obey_native_face_normals`
and then all three affected production contact/Twin gates were rerun.

The same production binary ran headfully on owned port 4142. The impact's nine
checks passed, readiness settled without faults or pending work, and a viewport
screenshot was inspected. API `Exit` returned accepted, the process exited zero
and its port was released. The headful and headless hosts do not constitute an
identical-input replay comparison.

## Measured performance limitation

In the final single-thread rover run, sampled physics steps during contact were
18.37–19.22 ms, including 15.80–16.20 ms in sweeps; later samples were 11.98 ms.
Impact contact samples also exceeded a 16.67 ms step budget. Headful settled
physics sampled 10.93 ms, but the viewport remained slow. Another simulator was
running concurrently and was left untouched. These are diagnostic samples,
not an uncontended FPS result or a proof of worst-case timing. The geometry
gates establish bounded physical behavior; further performance work must retain
those gates and use Tracy attribution plus separate clean runtime acceptance.
