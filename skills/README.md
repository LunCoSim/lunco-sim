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
| [**use-asset-library**](use-asset-library/SKILL.md) | Add a component, shader, Modelica model, or event-driven Rhai policy to `assets/` and have the engine find it |

## Author the world & its behaviour

| Skill | Use it when you want to… |
|---|---|
| [**geo-assets**](geo-assets/SKILL.md) | Put REAL lunar ground in a scene — download an LROC/PDS DTM, bake heightmap + colour/normal/slope maps, wire them as terrain layers |
| [**author-usd-component**](author-usd-component/SKILL.md) | Model a reusable `.usda` asset from scratch — geometry, material, physics, parameters, spawn catalog |
| [**build-vehicle**](build-vehicle/SKILL.md) | Assemble a rover/vehicle from the mobility component library — wheels, tires, suspensions, chassis, variant axes, drive laws, live tuning |
| [**build-usd-scene**](build-usd-scene/SKILL.md) | Assemble a scene from assets that already exist — load, spawn, place, and tune objects |
| [**author-usd-physics**](author-usd-physics/SKILL.md) | Author physics in USD — joints and joint FRAMES, gravity per scene, why a mechanism is rigid, a vehicle flies apart, or a part falls off it |
| [**author-scenario**](author-scenario/SKILL.md) | Write rhai behaviour — missions, waypoints, reactions, multi-entity coordination |
| [**authoring-vessel-controllers**](authoring-vessel-controllers/SKILL.md) | Give a vessel a self-driving GNC / autopilot with manual handoff |
| [**compose-multidomain-twin**](compose-multidomain-twin/SKILL.md) | Assemble a full mission — USD + Modelica + cosim + rhai — into a Twin |
| [**author-tutorial**](author-tutorial/SKILL.md) | Build a guided interactive lesson / onboarding flow (rhai + teaching HUD) |

## Run, observe & verify

| Skill | Use it when you want to… |
|---|---|
| [**run-modelica**](run-modelica/SKILL.md) | Run / compile / sweep Modelica models over the HTTP API |
| [**inspect-simulation**](inspect-simulation/SKILL.md) | Observe a running sim — read ports/variables, screenshot the viewport |
| [**record-video**](record-video/SKILL.md) | Record deterministic video/PNG takes — windowed or windowless (`--offscreen`), CLI or rhai-sequenced |
| **produce-episode** (in `lunco-marketing/.claude/skills/`) | Cut a finished campaign video from a take — narration, Kdenlive assembly, master, grade |
| [**test-via-api**](test-via-api/SKILL.md) | Verify a change end-to-end via the API instead of asking the user to click |
| [**validate-assets**](validate-assets/SKILL.md) | Pre-flight a `.mo`/`.usda`/`.wgsl`/`.rhai` — does it parse, and is it *right*? — in seconds, with no app, window or GPU; plus `RunLint` for the loaded scene and where lint rules are authored |

## Extend the engine

| Skill | Use it when you want to… |
|---|---|
| [**usd-projection**](usd-projection/SKILL.md) | Work ON the USD layer — teach it a new prim type or attribute, or fix an edit that saved but didn't show up |
| [**visualize-physics-with-shaders**](visualize-physics-with-shaders/SKILL.md) | Make a simulated value VISIBLE — a strut that reddens under load, a tyre that glows where it slips |

## Build workbench UI

| Skill | Use it when you want to… |
|---|---|
| [**lunco-ui**](lunco-ui/SKILL.md) | Build workbench panels using the reactive `Panel`/widget patterns |
| [**lunco-theme**](lunco-theme/SKILL.md) | Use the centralized design tokens (colours, schematic palette) |

## Work at scale

| Skill | Use it when you want to… |
|---|---|
| [**deep-audit**](deep-audit/SKILL.md) | Audit the workspace across domains with parallel reviewers, then execute the fixes as a no-shim migration plan |
| [**subagent-batches**](subagent-batches/SKILL.md) | Run a multi-finding fix sweep with parallel agents on disjoint file lots — agents never build; the coordinator verifies once |

## Cross-cutting conventions (baked into every skill)

- **Use the built production binary**: build in the main worktree, then invoke
  `target/debug/luncosim` directly for validation, tests, and launches. Do not
  use the former `sandbox` name or substitute `cargo run` for the built binary.
- **Always launch luncosim with its HTTP API**: `target/debug/luncosim --api 4101` (use
  another explicit free port when needed). The MCP bridge's old default (3000) is stale;
  every controllable, visual, realtime, or scene-test luncosim process must carry
  an explicit `--api PORT`. Only parse-only `--validate` invocations are exempt.
- **Exit the previous session before launching the next**: send the API `Exit` command,
  verify the process and port are gone, then start the replacement. Never overlap luncosim
  GUI/API sessions or reuse a port while the old session is still alive.
- **curl-first** over the `mcp__lunco__*` tools; drive the app over `POST /api/commands`.
- **Discover, don't hardcode** the command set — `DiscoverSchema` enumerates it live.
- **Policy → rhai, identity → USD, math → Modelica** — keep logic out of the Rust core.
- **USD is the source of truth; the ECS is a projection of it.** An edit that
  doesn't lower to a `UsdOp` escapes save, journal, undo *and* replication —
  silently. See [**usd-projection**](usd-projection/SKILL.md).
- **Use the API `Exit`**, never `pkill`, to stop a running app.
- **Validate before you run.** `target/debug/luncosim --validate <files…>` parses assets in
  seconds with no GPU and catches broken references, missing wheel attrs and
  `if`/`when` in Modelica — **and runs the authored lint rules**, which is what
  reports a part that would fall off a vehicle. On a *loaded* scene use the verb:
  `cmd("RunLint", #{})` + `query("LintReport")`; nothing lints on its own. Rules
  are rhai (`assets/scripting/policy/lint_*.rhai`), one linter per domain, so a
  new rule is an edit, not a rebuild. See
  [**validate-assets**](validate-assets/SKILL.md) and
  [lint-substrate](../docs/architecture/lint-substrate.md).
- **Hierarchy is namespace; a joint is attachment.** A mounted part that applies
  `PhysicsRigidBodyAPI` and is jointed to nothing is a free body and falls out of
  the vehicle — silently, with every parity test green. See
  [**author-usd-physics** §6](author-usd-physics/SKILL.md#6-a-part-is-not-a-body).
- **Shipped assets are `@lunco://…@`.** A bare relative path resolves against the
  anchoring document, so it breaks once a Twin mounts the file — and for
  `info:sourceAsset` that failure is **silent**.
- **Colour is `primvars:displayColor`**, shader or not; WGSL opts in with
  `//!@engine display_color`.

New to the codebase? Start with [**repo-map**](repo-map/SKILL.md), then the
[Documentation Hub](../docs/README.md) and the [AI Agent Guide](../AGENTS.md).

## Writing or changing a skill

A skill is a **runbook**, not a design doc. It answers "walk me through doing
this"; the *why* lives in `docs/architecture/`, and the skill links to it.

- **The `description` is the trigger.** Write it in the words a user would
  actually use — "the rover flips over", not "vehicle stability analysis" — and
  include the mid-code tells an agent would notice. It is matched against the
  request; a description that only names the subsystem never fires.
- **Lead with the trap.** The value of a skill is what a competent agent would
  get wrong from general knowledge alone. If everything in it is derivable from
  the docs, it should be a doc.
- **Every claim must be checkable** — a real path, a real command, a real
  attribute name. A skill that drifts is worse than none, because it is trusted.
- One skill per task shape. If two skills would trigger on the same request,
  merge them or make one defer to the other in its description.
- Every skill belongs in a table above and stays listed there.

`skills/` is symlinked as `.claude/skills/`, so these load automatically in
Claude Code.
