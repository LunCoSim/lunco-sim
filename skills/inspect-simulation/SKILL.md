---
name: inspect-simulation
description: >
  How to OBSERVE a running LunCoSim without asking the user to look — read live
  state, telemetry ports, cosim/Modelica variables, and viewport screenshots
  over the API. USE THIS SKILL whenever you need to answer "is it actually
  moving / working?", "what's the battery / altitude / speed / temperature?",
  "read the <X> port", "watch these signals over time", "what are the current
  variable values?", "did the cosim chain (Modelica → physics) work?", "what's
  in the scene right now?", or "show me what it looks like". Also trigger when
  you catch yourself about to `tail -f` the log and poll for a value, sleep-loop
  waiting for something to change, or ask the user "can you check?". The move is
  always: list entities → read the ports/variables → (watch if you need a
  series) → screenshot to confirm. It's the READ-side complement to
  test-via-api (which drives + verifies) and build-usd-scene (which authors).
  Project-specific: entity `api_id`s come from `list_entities`, ports are
  name-addressed and huge unless filtered, telemetry lives on the same API port
  (4101), and log-polling is the wrong tool — the port snapshot is authoritative.
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

## Example (curl)

```bash
# what's spawned?
curl -s -X POST http://127.0.0.1:4101/api/commands -H 'Content-Type: application/json' \
  -d '{"type":"ListEntities"}'
# the lander's altitude + descent-rate ports (filtered)
curl -s -X POST http://127.0.0.1:4101/api/commands -H 'Content-Type: application/json' \
  -d '{"command":"ReadPorts","params":{"name_filter":"Lander","ports":["altitude","descent_rate"]}}'
# confirm visually
curl -s -X POST http://127.0.0.1:4101/api/commands -H 'Content-Type: application/json' \
  -d '{"command":"CaptureScreenshot","params":{}}' -o /tmp/x.png   # then Read /tmp/x.png
```

## Gotchas

- **`read_ports` without an `api_id` is huge** — always `name_filter` and/or `ports`.
- **`api_id` (API-stable) ≠ the rhai `GlobalEntityId`** — get `api_id` from `list_entities`, don't reuse a gid from a script.
- **Port not found / empty?** The entity may be pre-compile (Modelica hasn't produced variables yet — `cosim_status` shows nulls until it does), or the name is a USD-path substring you haven't matched. List its ports first with `read_ports {api_id}` (no `ports` filter) to see the real names.
- **Wrong port?** The MCP bridge historically defaulted to 3000 — the canonical API port is **4101**; set `LUNCO_API_PORT=4101` if the MCP tools miss.
- **Don't restart to "get clean state"** — read the running instance; see the ⚠️ in [`test-via-api`](../test-via-api/SKILL.md).
- **One-shot vs series:** `read_ports` samples once (call again for fresh values); use `watch_ports` for a time-series — don't sleep-loop `read_ports`.
