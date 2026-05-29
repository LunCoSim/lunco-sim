# ROS2 integration — sync via a bridge, not a new mechanism

How rover sync works when ROS2 is in the loop. Short version:

> **ROS2 is not a new sync mechanism (no M8).** It's a **server-side bridge** at the
> authority boundary that maps our existing mechanisms — **M2** (state), **M3**
> (commands), **M6** (clock) — onto ROS2 pub/sub / services / actions. A ROS2 node
> is just **another participant**: a *controller* (drives a vessel) and/or an
> *observer* (consumes telemetry). The internal world (M1–M7,
> `SYNC_ARCHITECTURE.md`) is unchanged. This is the same pattern as the README's
> CCSDS/YAMCS bridge — a different external protocol on the same edge.

---

## 1. Where the bridge sits

```
  browser / desktop clients ──M2/M3/M4──┐
                                        │
                              ┌─────────▼──────────┐
                              │   SERVER (authority)│   physics + cosim + sim clock
                              │   ECS world         │
                              └───┬──────────────┬──┘
              our wire (M2/M3/M6) │              │  lunco-ros bridge (server-only plugin)
                                  │              │  reads/writes the SAME ECS state
                                  │         ┌────▼─────┐
                                  │         │  DDS      │  ◀── ROS2's own transport+discovery
                                  │         └────┬─────┘
                                  ▼              ▼
                            (game clients)   ROS2 nodes: nav2, perception,
                                             control, SpaceROS — possibly other machines
```

- The bridge runs **only on the server** (where authority, avian, and Modelica
  live). It is **native-only** — never wasm. Browser/desktop clients never speak
  ROS2; they see ROS-driven entities through plain **M2**, exactly like any other
  server-authoritative motion.
- DDS does its own discovery and transport, so ROS2 nodes can be on other machines
  on the LAN — we don't route them through our wire. The bridge is a ROS
  participant; our `TransportKind` set is untouched.

---

## 2. A ROS2 node is just a participant (maps to existing concepts)

| ROS2 node role | Our equivalent | Path |
|---|---|---|
| **Control node** publishing `/cmd_vel`, joint goals | a controller **possessing** a vessel | bridge subscribes → feeds the **same `DriveRover` / actuator** path as a human pilot (M4/M3). `NetworkAuthority.owner = ros_bridge_session`. |
| **Perception/observer** consuming `/odom`, `/tf`, sensors | an **observer** | bridge publishes telemetry derived from authoritative state (M2). |

So when a ROS2 node drives the rover, human clients simply see it as
"possessed by the ROS controller" (observer view) — the **possession/authority
model already covers this**; the bridge is a `Session` like any other in the auth
layer.

---

## 3. ROS2 primitives ↔ our mechanisms

| ROS2 primitive | Dir | Maps to |
|---|---|---|
| Topic — sensor/telemetry pub (sim→ROS) | out | **M2** state: bridge reads authoritative components, publishes `sensor_msgs`, `nav_msgs/Odometry`, `geometry_msgs` |
| Topic — command sub (`/cmd_vel`, joints) | in | **M4 input / M3 command** → existing `DriveRover`/actuator observers |
| `/tf`, `/tf_static` | out | **M2 poses** (cell+transform → TF tree) + **M1** static structure |
| `/clock` (+ `use_sim_time`) | out | **M6** — our sim clock *is* ROS time |
| Service (request/response) | both | **M3** command with **`Ack`** (the `Mutation<P>`/`Ack` envelope is request/response already) |
| Action (goal / feedback / result) | both | **M3** goal + **M2** feedback stream + **M3** result (long-running, e.g. "navigate to") |
| Parameters | both | **M3** `ParameterChanged` |
| QoS (reliable / best-effort, durability) | — | mirrors our channel delivery: reliable→M3, best-effort→M2/M4 |

Nothing here needs a mechanism we don't have. The bridge is pure translation.

---

## 4. The two real translation problems (everything else is mechanical)

### A. Coordinate frames — the ROS analog of our floating-origin problem
ROS expects a **TF tree** with REP-103/REP-105 conventions (right-handed, x-forward
z-up; metric-scale coords near the frame origin) — `map → odom → base_link → …`.
Our world is `big_space` cell+offset in Bevy's convention.

**This is the same rebasing we already do per client** (`DESIGN_GAPS.md` A): treat
the ROS `map` frame as "the ROS participant's floating origin," anchored at a chosen
cell. Then:
- `map` origin ← a cell origin (keeps nav-stack coords small & metric).
- `base_link` ← rover `(CellCoord,Transform)` rebased into `map`, axis-converted
  (Bevy↔REP-103).
- `/tf_static` ← the content-derived structure (M1): sensors/wheels relative to base.

So **ROS is "just another client with its own origin (the `map` frame)."** We reuse
the rebasing code; we add an axis/units conversion at the boundary.

### B. Time & real-time factor
- Publish **`/clock`** from M6; ROS nodes run `use_sim_time:=true` → everyone
  shares sim-time.
- **Real-time factor:** HIL with a real controller wants ~realtime. Decide the
  coupling: free-run (sim publishes /clock, nodes keep up best-effort) vs.
  controller-paced (sim waits for the control node's command each step — true
  hardware/software-in-the-loop lockstep). Start free-run + realtime cap.
- **Time-warp:** like KSP-in-MP, **disable warp while a ROS controller has
  authority** (a nav stack can't be fast-forwarded), or warp /clock and accept the
  controller may not follow. Recommend: warp forbidden when a ROS session owns a
  vessel.

---

## 5. Implementation shape
- A server-side `lunco-ros` plugin (Layer 2b, like `lunco-networking`),
  feature-gated, **native-only**. Reads/writes the same ECS state; no domain crate
  imports it (same invariant as the networking backend).
- **In-process via `rclrs`** (Rust ROS2 client) is the lean default — direct ECS
  access, no extra serialization hop. Alternative: a **separate bridge process**
  talking our API on one side and ROS2 on the other (better isolation / decoupled
  ROS distro lifecycle, at the cost of a hop). Choose in-process unless ROS distro
  coupling becomes a problem.
- Standard message families: `rosgraph_msgs/Clock`, `tf2_msgs/TFMessage`,
  `nav_msgs/Odometry`, `geometry_msgs/Twist`, `sensor_msgs/*`,
  `control_msgs`/actions. SpaceROS uses the same — relevant for the space angle.

---

## 6. How this stays consistent with the architecture
- **No new sync mechanism, no new authority model.** ROS = an edge bridge; the
  bridge holds a `Session` and (when controlling) a `NetworkAuthority`.
- **Selection procedure unchanged** (`MECHANISM_SELECTION.md`): a new ROS-exposed
  signal is classified internally as usual (telemetry→M2, command→M3/M4); the
  bridge only decides *which topic* mirrors it. "Should this be a ROS topic?" is a
  **bridge-mapping** question, not a mechanism question.
- **Bandwidth/clients unaffected:** game clients never see ROS traffic; DDS carries
  it separately.
- Parallel to the README's **CCSDS/YAMCS** bridge — same boundary, different
  external protocol. Both are server-side, native-only adapters over the same M2/M3/M6.

---

## 6b. Copper (cu29) — Rust-native robotics runtime as an alternative/additional bridge

[Copper](https://github.com/copper-project/copper-rs) (cu29) is a **deterministic
robotics *runtime*** in Rust (task graph + structured logging + record/replay,
sub-µs latency, zero-alloc, data-oriented). As of v1.0-rc1 it extends **strict
determinism to *distributed* robots** — replaying systems spanning many computers
and MCUs as one unified execution, scaling to fleets. It is a peer to ROS2 on the
*robotics-runtime* edge — **not** a networking backend for our game clients, and
**not** a replacement for M2/M4/M6.

What it is / isn't (verified, v1.0-rc1):
- **Distributed-deterministic, but in the lockstep family**: it assumes **every
  node runs Copper**, on controlled hardware/networks (computers, MCUs, fleets),
  for unified deterministic execution + 100%-fidelity replay.
- **Native — incl. embedded/MCU. No wasm/browser.** Our clients are browsers.
- Its determinism = reproducible execution/replay of a *cooperating Copper system*,
  **not** clock-sync + prediction for *untrusted heterogeneous* peers over a lossy
  WAN.

**Copper's distributed clock ≠ our M6 — different problems, different families:**

| | Copper (distributed determinism) | Our **M6** |
|---|---|---|
| Peers | cooperating Copper nodes (native/MCU) | untrusted heterogeneous (browser+desktop+server) |
| Network | controlled (robot bus / LAN) | lossy WAN, NAT, packet loss |
| Model | **lockstep** / deterministic replay | server-authoritative + **prediction** (client runs ahead) |
| Goal | unified execution + exact replay | hide latency, converge despite loss |

So Copper does **not** fill the backend-decision gap (`STACK_COMPARISON.md`) and
does **not** revive lockstep *for us*: it can't run in the browser, and our world
(avian + async Modelica) isn't a Copper-deterministic system. The two reasons
lockstep was ruled out (browser clients, non-deterministic physics/cosim) still
hold.

**Where it fits:** exactly like the ROS2 bridge — a **server-side, native-only**
adapter. A Copper pipeline acting as a controller is *another participant* that
possesses a vessel and drives the same `DriveRover`/actuator path (M4/M3); telemetry
flows out via M2. The bridge generalizes: ROS2 *or* Copper (*or* both) plug into the
same authority boundary.

**Synergy worth using:** if the rover's *control software* is built in Copper, it's
a compelling HIL/SIL story — Copper deterministically runs & replays the whole
control system while our sim is the physics. Bridge it exactly like ROS2: drive
Copper's clock from **our M6 sim-clock** (the `use_sim_time` / `/clock` pattern) so
controller and sim share time, and let Copper's deterministic replay cover the
**control side** while M2/M3 carry state/commands across the authority boundary.
Copper's clock is internal to *its* deterministic system; M6 remains the
cross-peer/cross-runtime clock.

## 7. Open questions — **staged 2026-05-29 (see `DECISIONS.md` › ROS2)**
1. **Coupling** → free-run + realtime cap first; controller-paced lockstep only if a real controller needs it.
2. **rclrs in-process vs. separate process** → in-process default, unless ROS distro lifecycle coupling becomes painful.
3. **Authority arbitration** (human + ROS both want a vessel) → deferred; possession model handles single-owner. Co-control is post-MVP.
4. **Frame/units policy** → REP-103/105 + metric; concrete `map`-anchor cell confirmed at the ROS phase.
5. **Warp policy with ROS in the loop** → forbidden when a ROS session owns a vessel. → **D5**
