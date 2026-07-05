# LunCoSim Skills

Task-oriented runbooks for driving and extending LunCoSim — written for **AI
agents** (and useful to contributors). Each skill triggers on a kind of request,
distills the relevant docs into a recipe, and bakes in the project-specific
gotchas so the happy path just works.

Each `SKILL.md` has a `description` with the phrases that trigger it; an agent
harness matches the request and loads the skill automatically. You can also read
one directly when doing that kind of task by hand.

## Orientation

| Skill | Use it when you want to… |
|---|---|
| [**repo-map**](repo-map/SKILL.md) | Get your bearings — repo layout, which binary to run, where a feature lives |

## Author the world & its behaviour

| Skill | Use it when you want to… |
|---|---|
| [**build-usd-scene**](build-usd-scene/SKILL.md) | Author/edit the 3D world — load scenes, spawn, place, and tune objects |
| [**author-scenario**](author-scenario/SKILL.md) | Write rhai behaviour — missions, waypoints, reactions, multi-entity coordination |
| [**authoring-vessel-controllers**](authoring-vessel-controllers/SKILL.md) | Give a vessel a self-driving GNC / autopilot with manual handoff |
| [**compose-multidomain-twin**](compose-multidomain-twin/SKILL.md) | Assemble a full mission — USD + Modelica + cosim + rhai — into a Twin |
| [**author-tutorial**](author-tutorial/SKILL.md) | Build a guided interactive lesson / onboarding flow (rhai + teaching HUD) |

## Run, observe & verify

| Skill | Use it when you want to… |
|---|---|
| [**run-modelica**](run-modelica/SKILL.md) | Run / compile / sweep Modelica models over the HTTP API |
| [**inspect-simulation**](inspect-simulation/SKILL.md) | Observe a running sim — read ports/variables, screenshot the viewport |
| [**test-via-api**](test-via-api/SKILL.md) | Verify a change end-to-end via the API instead of asking the user to click |

## Build workbench UI

| Skill | Use it when you want to… |
|---|---|
| [**lunco-ui**](lunco-ui/SKILL.md) | Build workbench panels using the reactive `Panel`/widget patterns |
| [**lunco-theme**](lunco-theme/SKILL.md) | Use the centralized design tokens (colours, schematic palette) |

## Cross-cutting conventions (baked into every skill)

- **API port is 4101** (`--api`); the MCP bridge's old default (3000) is stale.
- **curl-first** over the `mcp__lunco__*` tools; drive the app over `POST /api/commands`.
- **Discover, don't hardcode** the command set — `DiscoverSchema` enumerates it live.
- **Policy → rhai, identity → USD, math → Modelica** — keep logic out of the Rust core.
- **Use the API `Exit`**, never `pkill`, to stop a running app.

New to the codebase? Start with [**repo-map**](repo-map/SKILL.md), then the
[Documentation Hub](../docs/README.md) and the [AI Agent Guide](../AGENTS.md).
