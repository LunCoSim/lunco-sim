# Networking — resolved decisions (canonical log)

This file records the current ownership and integration decisions. Runtime
mechanisms are detailed in `SYNC_ARCHITECTURE.md`; deployment and validation
procedures are maintained in `DEPLOY.md`.

---

## D1 — Backend: Lightyear transport and messaging

Lightyear 0.31 provides WebTransport, netcode connections, typed message channels
and connection clock synchronization. The workspace disables its optional ECS
replication, prediction and interpolation features. `lunco-networking` ferries
LunCoSim's versioned `SyncEnvelope` through one `Frame` message on the declared
command, snapshot and bulk channels.

`lunco-networking-sync` owns snapshot capture/application, identity translation,
authorization, journals and asset transfer. `lunco-networking-core` owns physics
prediction and reconciliation. These owners share the same contracts on native
and browser targets. Validate the current composition through the bounded
host/client smoke and the browser build/runtime gates described in `DEPLOY.md`.

## D2 — Reconciliation: LunCoSim owns Avian state and control replay

Locally predicted bodies run the application's Avian dynamics. Snapshot
acknowledgements identify the committed control sequence used for reconciliation;
correction changes physics state through the networking prediction owner.
Ordinary owned-body reconciliation is the default. `LUNCO_ROLLBACK=1` explicitly
enables bounded assembly replay over unacknowledged committed control frames.
Replay restores only the enrolled control inputs and controller holds, preserving
unrelated authoritative state; an incompatible input surface rejects visibly.

LunCoSim's typed snapshot and control-frame owners remain authoritative regardless
of the transport dependency's optional physics integrations. Do not install a
second ECS replication or interpolation owner. See README → *Client-Side
Prediction* and `SYNC_ARCHITECTURE.md` for membership and correction boundaries.

## D3 — Identity: **deterministic from provenance** (confirmed)
Network id = pure function of provenance. Content/Derived → deterministic hash
(local spawn, not replicated); Authoritative → server-allocated + spawn replicated;
Local → never networked. Unreal net-stable-names / content-GUID model. The logic
lives in `lunco-core` (see README → *Entity Identity Mapping*).

### D3a — Collision policy: **53-bit on the sync layer + debug-time collision check**
Keep ids in the JS-safe 53-bit space (hard browser constraint). Add a debug-time
collision check at content load. Do **not** build 128-bit-internal/narrow-on-the-sync-layer
machinery now — per-scene populations are tiny (5000-prim sample: zero collisions);
birthday-bound at 53 bits is comfortable for thousands of entities. Revisit only if
a real scene approaches ~10⁶ entities.
- Supersedes IDENTITY Q1.

### D3b — `SourceId`: **logical scene name for identity, content-hash for cache**
Identity keys on the stable logical scene name + canonical prim path, so a USD edit
does **not** reassign ids (path-based id is exactly why live edits keep identity
stable — IDENTITY Q3 confirmed *yes*). The content-hash is a separate concern, used
only for asset fetch/dedupe.
- Supersedes IDENTITY Q2 and Q3.

## D4 — Spawn authority: **content = local spawn + deterministic id; runtime = server spawns + replicates**
The Unreal level-actor vs dynamic-actor split. Content-instanced entities are
spawned locally on each peer (only their *state* replicates); runtime-born entities
are spawned by the server, which allocates the id and replicates the spawn.

## D5 — Time-warp in multiplayer: **host-only, applied to all; forbidden when ROS owns a vessel**
- MVP: only the host may warp/pause; it applies to the whole shared world (server
  owns the clock). Per-client independent warp is **never** allowed (desyncs by
  definition).
- When a ROS controller holds authority over a vessel: warp is **forbidden** (a nav
  stack can't be fast-forwarded). Hard rule.
- Matches KSP (forbids MP warp) / Factorio (speed tied to lockstep, voted).

## D6 — Clock seam: transport and simulation retain their owners

Lightyear's connection timelines pace the transport and synchronize network
connections. `SimTick` identifies authoritative simulation boundaries and travels
in LunCoSim's snapshot and control-frame contracts. Connection clocks do not
replace the simulation clock or independently admit simulation work. Local
pause/rate and committed scene-generation rules remain at their runtime owners.

## D7 — Networking is an opt-in Cargo feature

The `networking` feature enables the optional Lightyear dependency and its
transport adapter. Default local launches do not create a multiplayer host.
`lunco-core` and `lunco-core-session` retain identity, command-origin, authority,
ordered-input and lifecycle contracts independently of the transport. Domain
crates consume those typed contracts rather than importing Lightyear.

---

## ROS2 — staged (not blocking; confirm at the ROS phase)
- Coupling: **free-run + realtime cap** first; controller-paced lockstep (true HIL)
  only if a real controller needs it. (ROS2_BRIDGE Q1)
- Integration: **rclrs in-process** (direct ECS, no extra hop) unless ROS distro
  lifecycle coupling becomes painful. (ROS2_BRIDGE Q2)
- Human+ROS authority arbitration: **deferred** — possession model already handles
  single-owner; co-control is a post-MVP feature. (ROS2_BRIDGE Q3)
- Frames/units: REP-103/105 + metric, `map` frame anchored at a chosen cell —
  confirm concrete anchor at the ROS phase. (ROS2_BRIDGE Q4)

## Deferred (acknowledged, standard practice)
Rover-rover collision under prediction, lag compensation, input validation/anti-cheat,
interest management, compression stack. Source/Overwatch don't predict pawn-vs-pawn
either — these are correctly out of MVP scope.

---

## Tuning boundaries

Transport latency, redundant input transmission and queue budgets require
measurements through the real host/client path. They do not change ownership of
snapshot coordinates, committed input ordering or physics interpolation. Current
mechanisms and remaining delivery gaps are documented in `SYNC_ARCHITECTURE.md`
and README → *Known gaps*.
