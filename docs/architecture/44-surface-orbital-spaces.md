# 44 — Surface and orbital reference frames

> Status: Active · Audience: contributors implementing views, trajectories,
> terrain, or body-relative vehicles.

Surface and orbital views are camera projections of the same semantic frame
tree. They are not separate ad-hoc coordinate systems and they do not move the
world root to follow the camera.

## Frame selection

The runtime uses named `ReferenceFrame` values:

- surface work belongs to `BodyFixed { body }`;
- body-centred orbital work belongs to `EclipticJ2000 { center: body }`;
- solar-system work belongs to the inertial solar/barycentric frame;
- scene-only work belongs to `World` until an authored physical anchor selects
  a celestial frame.

The `ReferenceFrameIndex` maps each semantic frame to one concrete BigSpace
grid. The index fails closed for missing or duplicate declarations. User-facing
commands name a target/frame; they do not name a grid or write a cell.

## BigSpace hierarchy

The persistent root is one `BigSpace + Grid` hierarchy:

```text
WorldRoot
└── Solar inertial grid
    ├── body inertial grid
    │   └── spacecraft / inertial trajectories
    └── body-fixed grid
        └── surface grid
            └── terrain, rovers, surface camera, surface trajectories
```

The body-fixed grid is the object that rotates. The body entity itself stays
identity-relative to that grid. An inertial sibling provides a stable frame for
star-fixed views and trajectories. A surface grid is a precision child of the
body-fixed frame; it does not introduce a second semantic identity.

## Switching views

1. A view command publishes a target and a semantic frame.
2. The camera system resolves the frame and computes the complete f64 pose.
3. The camera is attached to the destination grid with the atomic BigSpace
   mount operation.
4. The persistent `OriginAnchor` remains the sole `FloatingOrigin` owner. The
   selected camera's f64 pose updates the anchor cell in `WorldGrid`; no parent
   grid is re-posed and no next-frame correction is scheduled.

Surface orientation is derived from the local gravity/up direction and the
selected frame. Orbital orientation is derived from the inertial frame. This
keeps the ground below the view while preserving the Moon/Earth orientation
when the camera changes mode.

## Trajectories and links

Trajectory data carries an explicit semantic reference frame. Samples are
converted through the f64 frame transform and then emitted as grid-local,
cell-anchored render geometry. Line endpoints, labels, and connection beams
must use the same frame conversion as their source entities; never combine a
camera-relative `GlobalTransform` with a grid-absolute position.

## Physics and terrain

The active surface physics partition is explicitly bound through
`ActivePhysicsFrame`; the persistent shell does not select it implicitly. Avian receives f64 poses through
`BigSpacePhysicsBridgePlugin`; celestial parent motion does not become rover
motion. Terrain colliders and rover roots must share the active body-fixed
surface grid. A streamed tile is attached once to its owning grid, not repaired
by a per-frame offset.

## Extension rule

To add a new reference frame:

1. define its semantic identity and f64 transform in `lunco-celestial`;
2. add one concrete grid declaration and register it in the frame index;
3. route placement/camera/trajectory/physics consumers through the existing
   frame conversion and atomic mount APIs;
4. add a missing/duplicate-frame test and an end-to-end pose round-trip test.

Do not add a new camera-specific grid, raw coordinate field, fallback parent,
manual Euler correction, or per-frame repair loop.
