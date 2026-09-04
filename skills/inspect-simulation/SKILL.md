---
name: inspect-simulation
description: >
  Observe a running LunCoSim through its API: read entities, telemetry ports,
  Modelica or cosimulation variables, time series, and viewport screenshots.
  Use for questions about live values, motion, scene contents, or visual state.
  This is the read-only complement to test-via-api, which drives and verifies;
  use build-usd-scene when the task is authoring.
---

# Inspect a running simulation

Observe over the HTTP API / MCP — never by polling logs or asking the user. The
app must be running with `--api` (default port **4101**; launch per
[`test-via-api`](../test-via-api/SKILL.md)). Drive from curl `POST /api/commands`,
or the `mcp__lunco__*` tools if wired.

> **Read the ports, not the log.** A telemetry port snapshot is the authoritative
> current value. `tail -f`/`sleep`-polling a log for a number is the anti-pattern.

## The read surface

| Tool / query | Answers |
|---|---|
| `list_entities` (`ListEntities`) | every registered entity → `{api_id, name, type, pos}`. **Start here** — most reads need an `api_id`. |
| `query_entity` (`QueryEntity {id}`) | one entity's pose/name/type blob. |
| `QueryUsdPrim` | composed USD attributes and the resolved world position for a prim; use it to verify `xformOpOrder`, placement heading and mounted-part dimensions. |
| `InspectUsdViewport` | the explicit focused USD preview/view handles, document IDs, edit targets, projection generations, and `projection_ready` state; use this to identify the exact editor item visible in a screenshot and wait for a ready preview before editing. |
| `read_ports` | **live telemetry.** With `api_id`: that entity's ports `[{name,value,direction,kind}]`. Without: EVERY port-bearing entity (large — pass `name_filter` substring and/or `ports:[…]` to narrow). One-shot. |
| `read_port` `{api_id, port}` | a single named port value. |
| `watch_ports` `{api_id, …}` | a **time-series** of ports (use when you need change over time, not a single sample). |
| `snapshot_variables` (`SnapshotVariables`) | current Modelica variable values (the solver's state). |
| `cosim_status` | every USD-driven cosim entity end-to-end: `{name, y, vy, netForce, force_y_input, buoyancy, modelica_*}` — verify a **Modelica → physics** chain without logs. |
| `rover_status` | rover-specific convenience readout. |
| `capture_screenshot` (`CaptureScreenshot`) | raw PNG — save `-o /tmp/x.png`, then Read it. Confirms what numbers can't (did it tip over?). |

To perturb-then-observe: `set_input` / `SetPorts {target, writes:[[name,val]]}` to
poke an input, `possess_vessel` to take control, then re-read.

## Recipe

1. `list_entities` → find the `api_id` of the thing you care about (by `name`).
2. `read_ports {api_id, ports:[…]}` (or `read_port`) for the value(s) — filter, don't dump.
3. Need a trend (settling, oscillation, arrival)? `watch_ports` for a series instead of hammering `read_ports`.
4. Modelica in the loop? `snapshot_variables` for solver state, or `cosim_status` for the whole chain.
5. `capture_screenshot` → `/tmp/x.png` → Read it, to confirm the physical picture.

If the scene has no authored window camera, the windowed luncosim host may use
its explicit standalone presentation policy after finite USD bounds settle; it
reports the generated owner in the Camera menu and status history. Headless and
recording hosts do not opt into that policy. A transient camera-less projection,
or a boundless standalone scene, is a presentation diagnostic rather than a
terrain or physics failure.

Before interpreting a live read as a finished scene, check
`GET /api/ready` and require `ready:true`, `world_hold:false`, and
`pending_count:0`. A port may be absent while its Modelica island is still
compiling; that is different from a valid zero.

For DEM terrain, read the typed `TerrainLodStatus` query as the authoritative
geometry stream state. A settled visual terrain requires `wanted == resident`
and `pending == 0`; the status-bar text is presentation history and is not a
readiness signal. Streamed Lit terrain also waits for its USD material source
projection and any required off-thread derived surface/normal product before
exposing the initial tile set. During startup, a status entry may intentionally
remain live at `resident == wanted` while `pending > 0`: that is render-material
publication, not a completed tile bar. Observe the live `terrain-derived` status
entry and the typed query rather than treating a historical terrain event as
proof that materials are settled. Static USD DEM terrain keeps its `UsdShade`
appearance intent on the terrain owner while the generated mesh is assembled.

For an interactive USD edit, query `InspectUsdViewport` before describing or
changing the visible item, then correlate its explicit view/preview handle with
`CaptureScreenshot` and `ListOpenDocuments`. Use the returned document ID,
edit target, and projected generation in typed USD commands; do not infer the
target from a tab title, file name, or entity name.

### Fixed-panel rover readout

For a fixed solar deck, list the rover-root network entity and read its
boundary ports plus the panel and battery member outputs. The useful minimum
is `solar_power`, `solar_incidence`, panel `power_out`/`generated_current_a`,
and battery `terminal_current_a`/`soc_out`. A positive mesh count or a visible
`SolarPanel` prim does not prove that current reaches the battery.

For a placed rover, pair the live pose with `QueryUsdPrim` on the composed rover
prim. Confirm the local forward-axis contract, the effective rotation op and
its `xformOpOrder`; do not infer orientation from a screenshot chosen on a
symmetry axis.

## Example (curl)

```bash
# what's spawned?
curl -s -X POST http://127.0.0.1:4101/api/commands -H 'Content-Type: application/json' \
  -d '{"type":"ListEntities"}'
# the lander's altitude + descent-rate ports (filtered)
curl -s -X POST http://127.0.0.1:4101/api/commands -H 'Content-Type: application/json' \
  -d '{"type":"ExecuteCommand","command":"ReadPorts","params":{"name_filter":"Lander","ports":["altitude","descent_rate"]}}'
# confirm visually
curl -s -X POST http://127.0.0.1:4101/api/commands -H 'Content-Type: application/json' \
  -d '{"type":"ExecuteCommand","command":"CaptureScreenshot","params":{}}' -o /tmp/x.png   # then Read /tmp/x.png
```

## Gotchas

- **Direction tracker**: inspect the entire coordinate chain together: the
  world-to-mount direction ports, controller setpoints, measured joint angles,
  and the rendered boresight. A `locked` or low-error controller output alone
  can validate the same incorrect frame convention that points the mechanism
  away from its target.

- **`read_ports` without an `api_id` is huge** — always `name_filter` and/or `ports`.
- **`api_id` (API-stable) ≠ the rhai `GlobalEntityId`** — get `api_id` from `list_entities`, don't reuse a gid from a script.
- **Port not found / empty?** The entity may be pre-compile (Modelica hasn't produced variables yet — `cosim_status` shows nulls until it does), or the name is a USD-path substring you haven't matched. List its ports first with `read_ports {api_id}` (no `ports` filter) to see the real names.
- **Wrong port?** The canonical API port is **4101**; set `LUNCO_API_PORT=4101` if the MCP tools miss.
- **Don't restart to "get clean state"** — read the running instance; see the ⚠️ in [`test-via-api`](../test-via-api/SKILL.md).
- **One-shot vs series:** `read_ports` samples once (call again for fresh values); use `watch_ports` for a time-series — don't sleep-loop `read_ports`.
