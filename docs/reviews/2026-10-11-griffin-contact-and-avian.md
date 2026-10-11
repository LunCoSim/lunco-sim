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
Native contact samples validate both local witnesses and both unit normals.
The query adapter retains unsupported queries and incomplete unbounded results
across Parry's optional-hit boundary. Such failures cannot become clear-space
results; the continuous owner holds the affected tick before writeback.

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

Parry's newer published release is 0.31.1. Its native GJK correction fixes a
reproduced false-negative intersection between coincident overlapping cylinders
in our 0.27 dependency. The failing fixture exited 101 before the correction.
`third_party/parry3d-f64` retains published 0.27 source and the Bevy 0.19 / glam
0.32 type contract, incorporating the published 0.31.1 GJK implementation and
the upstream scalar-math enhanced-determinism feature correction. Source archive
hashes, licensing and verification requirements are recorded in its provenance.
This is a native math correction; it does not substitute scene geometry or
alter authored contact tolerances.

Scalar math exposed missing aligned-mask serialization in glam 0.32.1, preventing
Bevy reflection from compiling. `third_party/glam` retains the published native
types and implements their tuple serialization in both scalar and SIMD builds.
It also corrects `BVec4A` fourth-lane serialization and the `BVec3A` tuple visitor
name. All sixteen mixed four-lane masks roundtrip with their exact wire values.
The root lock retains existing Windows dependency resolutions. Full dependency
upgrades and cross-platform deterministic replay require their own verification;
the local tests do not establish either.

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

The final headless impact measured maximum native penetration 2.268212 mm,
minimum sampled pad clearance -2.367628 mm, hard-stop error 4.371646 nm and final
ground speed 1.522568 mm/s. The rover reached 2.454570 m/s and contacted the PX
shock-barrel tube collider, with maximum pair penetration 0.059945 mm and a
positive native collision impulse. No collision filters or authored mechanical
limits were weakened to obtain these verdicts.

Low-level tests cover contact trajectory bounds, SO(3) derivatives, finite mesh
features and native normal constraints, immutable contact-cache lifetime, local
property undo and resolved weak-property projection. The current native kernel
passed all eight `ccd::tests`, including the overlapping-cylinder fixture,
finite-edge/swapped-frame geometry and retained query errors. Scalar aligned-mask
tests passed the truth table and five native valid/malformed-tuple tests; SIMD
passed all six native boolean serialization tests. The production build and all
nine production gates were rerun with the native fixes and scalar math enabled.

The same production binary ran headfully on owned port 4142. The impact's nine
checks passed, readiness settled without faults or pending work, and a viewport
screenshot was inspected. API `Exit` returned accepted, the process exited zero
and its port was released. The headful and headless hosts do not constitute an
identical-input replay comparison.

## Measured performance limitation

The owned Tracy baseline attributes 5.183 seconds over 45,071 triangle-cylinder
casts to the native cast kernel during the 900-tick rover gate; total sweep time
is 13.437 seconds. These nested spans overlap and must not be added together.
The checked query adapter exposes contact and closest-point spans with native
shape-type fields for further attribution.

Unprofiled single-thread rover histories cover the same 541 physics steps,
ticks 240–780, with no sampling gaps. The normal baseline measured p50 14.189 ms,
p95 19.223 ms, p99 22.281 ms and maximum 23.627 ms. The final native correction
and scalar-math run, after compilation ended, measured p50 16.422 ms, p95
21.334 ms, p99 25.467 ms and maximum 37.431 ms; 257 steps exceeded 16.667 ms.
These changes fix correctness and deterministic math configuration; they do
not meet the real-time budget. Another simulator ran during the baseline and
was left untouched. The histories measure physics steps, not the complete fixed
transaction or uncontended viewport FPS. Further performance acceptance must
retain the geometry gates and measure the complete owner boundary.
The final headful impact confirms that limitation at the owner boundary:
the 240-tick fixed-transaction window measured p95 38.274 ms, with 148.064 seconds
of pending simulation time at capture. No simulation ticks were dropped to
hide this backlog. A ready world and a passing physical verdict do not establish
real-time cadence or UI performance.
