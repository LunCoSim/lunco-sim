# Applications & Binaries

Every runnable binary in the workspace, what it is, and how to launch it. This
is the index — each primary app has its own page under `docs/apps/<app>/` with
full CLI flags, controls, and workflows.

> **`cargo run` needs a target.** The workspace `default-members` are
> `luncosim`, `lunco-sandbox`, and `lunco-modelica`, so a bare `cargo run` is
> ambiguous. Always pass `-p <crate>` and/or `--bin <name>`.

## Primary apps

| Binary | Crate | Launch | What it is |
|---|---|---|---|
| `luncosim` | `luncosim` | `cargo run -p luncosim` | **Flagship simulator.** Full lunar mission: celestial bodies + ephemeris, solar-system-scale `big_space`, orbital camera, and the whole FSW / Hardware / Mobility / Robotics / Avatar stack under the workbench. Always windowed (native) or web (wasm). |
| `sandbox` | `lunco-sandbox` | `cargo run --release -p lunco-sandbox --bin sandbox` | **Physics Sandbox.** Ground mobility + physics test bed — collaborative 3D scene (USD + Avian3D). Windowed, headless (`--no-ui`), or web. See [sandbox](sandbox/README.md). |
| `sandbox-server` | `lunco-sandbox-server` | `cargo run -p lunco-sandbox-server` | **Headless server.** Same sim as `sandbox` via `run_headless()`, but the GUI stack (winit/egui) is never linked — for multiplayer hosting and automation. Deploy guide: [sandbox/OPS.md](sandbox/OPS.md). |
| `lunica` | `lunco-modelica` | `cargo run --bin lunica` | **Modelica engineering workbench.** Author, compile (rumoca), and simulate Modelica models; MSL browser. Windowed, headless (`--no-ui`), or web. See [lunica](lunica/README.md). |
| `lunco-assets` | `lunco-assets` | `cargo run -p lunco-assets --bin lunco-assets -- <download\|list\|process>` | **Assets Manager.** Download / verify (SHA-256) / process external assets (textures, MSL, models). See [assets-manager](assets-manager/README.md). |

## Utility & dev binaries

| Binary | Crate | Launch | What it is |
|---|---|---|---|
| `modelica_run` | `lunco-modelica` | `cargo run -p lunco-modelica --bin modelica_run` | Headless Modelica CLI — compile a model, step it for a fixed duration, optionally dump per-step variables to CSV. |
| `msl_indexer` | `lunco-modelica` | `cargo run -p lunco-modelica --bin msl_indexer` | Builds the Modelica Standard Library search index. Same entry the workbench drives in-process. Re-run after an MSL rebuild. |
| `lunica_worker` | `lunco-modelica` | (wasm only) | Off-thread rumoca compile worker for the web build. Not run directly — bundled by `scripts/build_web.sh`. |
| `build_msl_assets` | `lunco-assets` | `cargo run -p lunco-assets --bin build_msl_assets` | Bundles the MSL into shippable assets. |
| `net_smoke` | `lunco-networking` | `scripts/net_smoke.sh` | Networking transport smoke test. |
| `joint_minimal` | `lunco-sandbox` | `cargo run -p lunco-sandbox --bin joint_minimal -- --api 3001` | Minimal single-joint physics repro for debugging. |

`lunco-modelica` also carries bench/test bins (`modelica_tester`, `msl_parse_bench`, `test_within`) used during development.

## Web (wasm) builds

The windowed apps (`luncosim`, `sandbox`, `lunica`) share one desktop+web
source. Build the web bundle with:

```bash
scripts/build_web.sh build sandbox   # or: lunica
```

`lunco-web`'s `WebReadyPlugin` dismisses the HTML loader once the first frame
paints (no-op on native).

## Common CLI flags

These are honored by the windowed apps that embed the HTTP API bridge
(`sandbox`, `lunica`, and any app that installs `LunCoApiPlugin`):

| Flag | Effect |
|---|---|
| `--api [PORT]` | Enable the HTTP automation API. Omit `PORT` to use the default **4101** (`lunco_core::session::DEFAULT_API_PORT`). Without `--api`, no network surface is exposed. |
| `--no-ui` | Run headless — skip the winit window / egui chrome, run the shared sim/physics loop only. |
| `--scene <path>` | (`sandbox`) Load a USD scene on boot. Path is relative to the `assets/` source root — do **not** prefix with `assets/`. |

## Talking to a running app — HTTP API & MCP

Launch with `--api` and drive the sim over HTTP (POST to `/api/commands`) or via
the MCP server for AI agents.

- **Command format:** `{"command":"<Name>","params":{...}}`. Meta queries use
  `{"type":"DiscoverSchema"}` / `{"type":"ListEntities"}` / `{"type":"QueryEntity","id":<n>}`.
- **The command set is discovered, not hard-coded.** New `#[Command]` types
  self-register; enumerate the live surface with `DiscoverSchema` (HTTP) or the
  `discover_schema` MCP tool. Built-in read queries: `ListEntities`,
  `DiscoverSchema`, `QueryEntity`, plus extension queries (`ListBundled`,
  `ListOpenDocuments`, `ListTwin`, `ListMsl`).
- **Full API reference:** [`architecture/12-api.md`](../architecture/12-api.md)
  and [`crates/lunco-api/README.md`](../../crates/lunco-api/README.md).
- **MCP server:** [`mcp/README.md`](../../mcp/README.md) — wraps the HTTP API as
  MCP tools for Claude Desktop / Cline / Windsurf.

> **Port note:** the canonical default is **4101** everywhere (the `--api`
> default, the `scripts/api/*` helpers, and the docs). The bundled MCP config
> (`.mcp.json`, `mcp/mcp.json`) points at the same port via `LUNCO_API_PORT`.
> If you start the sim on a different port, set `LUNCO_API_PORT` to match.

## See also

- [`../crates-index.md`](../crates-index.md) — every library crate and what it owns.
- [`../../scripts/`](../../scripts/) — build, deploy, perf, and API helper scripts.
