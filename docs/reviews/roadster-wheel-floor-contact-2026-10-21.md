# Roadster wheel/floor contact reproduction

The replacement Roadster course is not qualified. A controlled standing launch,
with the course actor disabled and normal wheel/jet commands admitted, receives
a large nonvertical contact impulse while crossing the flat ground.

The reproduced baseline used Parry `fbef896f85d04dada74bec54ff7c64de77589705`.
The engine now selects published correction `97057ff03f733c34c96b1ffaff9092af4e9036a5`.
Application changes remain uncommitted. All 142 physics library tests pass
against the published correction, including the captured native regression.

## Native evidence

At completed cycle 192, `QueryPhysicsState` reports the rear-left wheel collider:

```
position = (-1.3184781679354403, 0.3402477450478824, -58.73657026013783) m
rotation_xyzw = (0.02337023784932594, -0.018544458633876493,
                 -0.0012613193118391128, -0.9995540726296082)
radius = 0.34 m; half width = 0.15 m; authored axle = X
```

The ground is a finite cuboid with half extents `(50000, 0.5, 50000)` m,
centered at `(0,-0.5,0)` m. The wheel is far from every floor edge. Its
analytical lower support is about `6.2e-13` m above the top face.

The production manifold prepared at 3.200000064 s instead supplies
`(-0.03630733570279097,-0.17030413011459292,-0.9847224383754424)`
as the wheel-to-ground normal. The cycle-193 contact receipt identifies
`/World/Roadster/Wheel_RL` against `/World/Ground` and reports about
80,380 N s of normal impulse. Motion changes abruptly in the same cycle.

## Independent regression

`lunco-physics::ccd::tests::native_wheel_floor_manifold_retains_the_floor_support_normal`
uses only `DefaultQueryDispatcher.contact_manifolds` and the captured geometry.
It checks both the native compound and the equivalent cylinder leaf, with
prediction distances 0, 0.001, 0.02 and 1 m.

All eight queries return a nonvertical normal. For the fresh compound query,
the ground-side normal is approximately `(-0.207354,0.147545,0.967075)`:
about 81.5 degrees away from the supporting floor normal. The reported witness
distance is `6.88e-12` m. The leaf reproduces the same defect, so compound
traversal alone does not explain it.

Run:

```
cargo test -p lunco-physics --lib \
  native_wheel_floor_manifold_retains_the_floor_support_normal -- --nocapture
```

The application-owned test and native receipts establish the defect without
changing dependency sources, replacing a normal or selecting another seed.
The failure belongs to GJK stagnation: a support-plane bound classifies the
near-touching pair while the returned normal came from a nearly zero simplex
projection. The support direction and the bound must stay paired. Below the
existing scale-relative boundary tolerance, the corrected kernel returns the
direction of the strongest sampled supporting bound. Well-separated queries
retain the converged witness direction. Shape geometry, seeds and predictions
are unchanged.

## Approved correction and qualification

The user's follow-up `continue` approved the explicitly requested focused fork
correction, its publication and engine pin. Commit
`97057ff03f733c34c96b1ffaff9092af4e9036a5` is published on the existing
`LunCoSim/parry` branch `fix/robust-convex-queries-0.27`; the remote SHA was read
back. No new repository or application-owned normal replacement was introduced.

The native regression failed on the baseline. It passes after correction,
including 1,008 manifold queries covering three floor extents, three cardinal
orientations, seven clearances/penetrations, compound and leaf cylinders,
both shape orders and four prediction distances. The eight existing convex
regressions also pass. Library tests pass: 62 in 3D f64, 63 in 3D f32 and 59
in 2D f64. Logs are retained as `~/.cache/parry-floor-{baseline,qualification,
library,other-scalars}.log`.

The engine's full physics library suite passes against the published pin
(`~/.cache/roadster-parry-integrated-core.log`). Native powered trials no longer
receive the catastrophic floor-normal impulse. That correction does not qualify
Roadster behavior or new presentation footage: the current course completes,
but lateral deviation reaches 11.22 m and fails the 0.75 m centering gate.
Matched zero-steer trials show substantially greater pre-obstacle drift with
suction enabled. Tire support, shaft torque feedback and steering response
remain under investigation; no lateral failure is evidence of a thermal limit.
