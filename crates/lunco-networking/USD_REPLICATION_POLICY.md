# USD Replication Policy

How a scene says **what** to network-replicate and **how**. The policy is **derived
from the USD scene by default** — author a normal USD physics scene and you get
correct multiplayer replication for free. The only thing you ever hand-author is an
**exception**, via the `lunco:net:*` attributes below.

This is the entity/state-replication contract (which bodies' poses sync, and how the
client treats them). It is distinct from command/op replication; see
`MECHANISM_SELECTION.md` (M1–M7) for the broader sync model.

## TL;DR for scene authors

- Make a body move in multiplayer? **Nothing to do** — every non-static rigid body
  replicates by default (host-authoritative; clients see a smoothly interpolated proxy).
- Chassis + jointed wheels (a *Physical* rover)? **Nothing to do** — the articulation
  is read from the standard USD joint graph; the whole assembly replicates per-link and
  the client renders the true articulation (no flip, no faked spin).
- A body driven by cosim/Modelica forces a client can't reproduce? **Nothing to do** —
  attaching a sim model marks it opaque automatically.
- Want to *opt a body out* of the sync layer, or *force* a non-default authority? Author one
  `lunco:net:*` attribute (below).

## Default derivation (no authoring)

| USD fact (already authored)                                   | Policy                                          | Internal markers stamped |
|---------------------------------------------------------------|-------------------------------------------------|--------------------------|
| Static collider / no `PhysicsRigidBodyAPI`                    | not replicated                                  | —                        |
| Any non-static rigid body                                     | server-authoritative; client interpolates proxy | `NetReplicate`          |
| Joint `physics:body0` target / `PhysicsArticulationRootAPI`   | articulated **root** (kinematic-proxy assembly) | `+ ArticulatedVehicle`   |
| Joint `physics:body1` target                                  | articulated **link** (wheel)                    | `+ ArticulatedLink`      |
| Bound to a cosim `SimComponent`                               | **opaque** — never client-predicted             | `+ NotPredictable`       |
| Runtime spawn (`SkipContentStamp`)                            | server-spawned + replicated                     | `SkipContentStamp + NetReplicate` |

Structure (root/link) is read from the standard USD physics schema — `PhysicsRevoluteJoint`
prims with `physics:body0` (chassis) and `physics:body1` (wheel) rel targets — in
`lunco-usd-sim`'s `process_usd_sim_prims` (Pass 1 joint scan + Pass-2 stamping). There is
**no runtime physics-graph heuristic** and no build-order side-effect.

## Overrides — author only for exceptions

Namespace `lunco:net:*`, read at load by `process_usd_sim_prims` (mapping unit-tested in
`net_override_markers`):

| Attribute                          | Type    | Effect |
|------------------------------------|---------|--------|
| `lunco:net:replicate = false`      | `bool`  | Opt this body OUT of replication (`NetExcluded`). Stays purely local/cosmetic. |
| `lunco:net:replicate = true`       | `bool`  | Explicit include (it is not an exclusion; the default already replicates bodies). |
| `lunco:net:authority = "server"`   | `token` | Default. Host-authoritative; client interpolates. |
| `lunco:net:authority = "predictable"` | `token` | Replicated + eligible for client prediction. (Today identical markers to `server`; rover prediction eligibility is still ownership-derived at runtime — reserved for a future static gate.) |
| `lunco:net:authority = "opaque"`   | `token` | Replicated but **never client-predicted** (`NotPredictable`). For bodies driven by forces a client can't reproduce. Cosim bodies get this automatically. |
| `lunco:net:authority = "local"`    | `token` | Not replicated (`NetExcluded`). Same as `replicate = false`. |

Example (exclude a purely cosmetic dynamic prop from the sync layer):

```usda
def Xform "DecorBanner" (apiSchemas = ["PhysicsRigidBodyAPI"])
{
    bool lunco:net:replicate = false
}
```

## What the markers mean (for engine devs)

Derived markers live in `lunco-core` (`session.rs`) and are read by the client proxy/
prediction systems in `lunco-sandbox-edit` (`commands.rs`):

- `NetReplicate` — host serialises this body's WORLD pose+velocity each snapshot
  (`gather_snapshot`); client pins it kinematic and drives it from the snapshot curve.
- `NetExcluded` — the membership pass `apply_net_replication` skips it (never `NetReplicate`).
- `ArticulatedVehicle` — articulated root; never single-body predicted. A remote rover is a
  fully pose-forced assembly (kinematic chassis + kinematic wheels, inter-link joints inert).
- `ArticulatedLink` — a replicated wheel; disambiguates it from the chassis so
  `maintain_owned_locally` skips it and `propagate_owned_to_wheels` mirrors the chassis's
  `OwnedLocally` onto it (so the rover you drive runs local physics on all links).
- `NotPredictable` — opaque body; the predictor never takes it over (it would diverge and
  rubber-band). Stamped at the cosim takeover (`lunco-usd-sim/src/cosim.rs`) or via
  `authority = "opaque"`.

## Pipeline

1. **Load (both peers, deterministic):** `process_usd_sim_prims` reads the joint graph +
   `lunco:net:*` and stamps the structural/override markers once per prim.
2. **Membership (each frame, re-asserting):** `apply_net_replication` adds `NetReplicate` to
   every non-static rigid body that isn't `NetExcluded`/`SkipContentStamp`. (Re-asserting
   because the avian `RigidBody` materialises a frame or more after the prim entity exists.)
3. **Runtime classification (client):** ownership/prediction markers (`OwnedLocally`,
   `PredictedDynamic`) are set by the prediction-membership systems, not by USD.
