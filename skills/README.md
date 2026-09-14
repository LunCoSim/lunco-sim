# LunCoSim Skills

Task-oriented runbooks for driving and extending LunCoSim — written for **AI
agents** (and useful to contributors). Each skill triggers on a kind of request,
distills the relevant docs into a recipe, and bakes in the project-specific
gotchas so the happy path just works.

Each `SKILL.md` has a `description` with the phrases that trigger it; an agent
harness that supports discovery may match the request and load the skill
automatically. You can always read one directly, and any MCP-compatible host
can use the same Markdown path without host-specific syntax.

New to LunCoSim? Start with [`START-HERE.md`](START-HERE.md). It explains how to
choose one primary skill, defer to supporting skills, use the project formats,
work with installed GitHub builds, create dynamic Rhai tools, and produce an
evidence-backed handoff.

## How to use a skill

Use a skill when the request changes, authors, validates, observes, or reviews
LunCoSim work. State the desired outcome in ordinary language, then point the
agent or host at one primary runbook:

```text
Use skills/build-usd-scene/SKILL.md as the primary runbook. Assemble the
existing rover and terrain assets into <scene>; keep authored topology in USD
and verify it through the production API.
```

The primary skill owns the workflow. Load a neighbouring skill only when the
primary runbook defers to its contract; arrows in the routing table are
progressive disclosure, not a request to load the entire catalogue. Before
declaring a feature absent, use [`capability-discovery`](capability-discovery/SKILL.md)
to search the skills, canonical docs, current owner source, registrations,
maintained dependencies, and live API surface when applicable.

### Skill session contract

Every maintained skill is a task runbook with a discoverable frontmatter
`description`, one authoritative owner, a “read first” path, concrete commands
or file locations, and bounded verification. A completed session reports:

```text
Primary skill -> supporting skill(s) -> owner/source -> files changed
             -> checks and runtime/visual evidence -> limits/blocker
             -> branch/commit -> next action
```

The catalogue check enforces the mechanical part of that contract: valid skill
frontmatter, unique names, index coverage, and resolvable local Markdown links.
It is available as `python3 scripts/validate_skills.py` from the repository
root. This Python command is only a repository documentation check; it does not
enable or imply the optional LunCoSim Python integration. It does not turn a
parse check into runtime evidence.

## Orientation

| Skill | Use it when you want to… |
|---|---|
| [**START-HERE**](START-HERE.md) | Route a new task, choose a primary skill, understand formats, or prepare a cross-host handoff |
| [**repo-map**](repo-map/SKILL.md) | Get your bearings — repo layout, which binary to run, where a feature lives |
| [**capability-discovery**](capability-discovery/SKILL.md) | Find an existing capability and its owner before calling a feature missing or adding a duplicate mechanism |
| [**use-asset-library**](use-asset-library/SKILL.md) | Add a component, shader, Modelica model, or event-driven Rhai policy to `assets/` and have the engine find it |
| [**luncosim-architecture**](luncosim-architecture/SKILL.md) | Design or review a reusable feature across USD, Modelica, Avian, Rust, and Rhai; adopt standard USD schemas and remove legacy paths |
| [**coordinate-frames**](coordinate-frames/SKILL.md) | Diagnose or implement BigSpace, reference-frame, camera, terrain, trajectory, or physics pose changes without raw-f32 or repair logic |
| [**sysml-requirements**](sysml-requirements/SKILL.md) | Author, validate, and run Twin-owned SysML v2 requirements and verification cases; understand the opt-in subset and Rhai bridge |

## Author the world & its behaviour

| Skill | Use it when you want to… |
|---|---|
| [**geo-assets**](geo-assets/SKILL.md) | Put REAL lunar ground in a scene — download an LROC/PDS DTM, bake heightmap + colour/normal/slope maps, wire them as terrain layers |
| [**author-usd-component**](author-usd-component/SKILL.md) | Model a reusable `.usda` asset from scratch — geometry, material, physics, parameters, spawn catalog |
| [**author-rhai-tool**](author-rhai-tool/SKILL.md) | Create/register reusable Rhai tool libraries for typed USD plans, component lints, inspection, and same-session tests |
| [**build-vehicle**](build-vehicle/SKILL.md) | Assemble a rover/vehicle from the mobility component library — wheels, tires, suspensions, chassis, variant axes, drive laws, live tuning |
| [**build-usd-scene**](build-usd-scene/SKILL.md) | Assemble a scene from assets that already exist — load, spawn, place, and tune objects |
| [**edit-usd-assembly**](edit-usd-assembly/SKILL.md) | Create or modify a reusable rover/lander assembly in a live headful Editor session, with screenshot review and user feedback |
| [**assembly-quality**](assembly-quality/SKILL.md) | Apply Editor-first, typed-USD, componentized geometry, placement, dimension, and visual-evidence gates to any assembly |
| [**update-documents**](update-documents/SKILL.md) | Update canonical docs, agent guidance, and skills without duplicating retired contracts |
| [**author-usd-physics**](author-usd-physics/SKILL.md) | Author physics in USD — joints and joint FRAMES, gravity per scene, why a mechanism is rigid, a vehicle flies apart, or a part falls off it |
| [**author-scenario**](author-scenario/SKILL.md) | Write rhai behaviour — missions, waypoints, reactions, multi-entity coordination |
| [**authoring-vessel-controllers**](authoring-vessel-controllers/SKILL.md) | Give a vessel a self-driving GNC / autopilot with manual handoff |
| [**compose-multidomain-twin**](compose-multidomain-twin/SKILL.md) | Assemble a full mission — USD + SysML + Modelica + cosim + rhai — into a Twin |
| [**author-tutorial**](author-tutorial/SKILL.md) | Build a guided interactive lesson / onboarding flow (rhai + teaching HUD) |

## Run, observe & verify

| Skill | Use it when you want to… |
|---|---|
| [**run-modelica**](run-modelica/SKILL.md) | Run / compile / sweep Modelica models over the HTTP API |
| [**inspect-simulation**](inspect-simulation/SKILL.md) | Observe a running sim — read ports/variables, screenshot the viewport |
| [**record-video**](record-video/SKILL.md) | Record deterministic video/PNG takes — windowed or windowless (`--offscreen`), CLI or rhai-sequenced |
| [**test-via-api**](test-via-api/SKILL.md) | Verify a change end-to-end via the API instead of asking the user to click |
| [**validate-assets**](validate-assets/SKILL.md) | Pre-flight a `.mo`/`.usda`/`.sysml`/`.kerml`/`.wgsl`/`.rhai` or an entire Twin namespace — does it parse, resolve, and lint correctly? — in seconds; plus `ValidateSysml`/`RunLint` for Twin and loaded-scene checks |

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
| [**runtime-ui**](runtime-ui/SKILL.md) | Author a reloadable Twin-facing HTML/CSS-like surface, bind engine capabilities, or add a semantic runtime UI action |

## Work at scale

| Skill | Use it when you want to… |
|---|---|
| [**deep-audit**](deep-audit/SKILL.md) | Audit the workspace across domains with parallel reviewers, then execute the fixes as a no-shim migration plan |
| [**subagent-batches**](subagent-batches/SKILL.md) | Run a multi-finding fix sweep with parallel agents on disjoint file lots — agents never build; the coordinator verifies once |

## Release & handoff

| Skill | Use it when you want to… |
|---|---|
| [**nightly-changelog**](nightly-changelog/SKILL.md) | Prepare a traceable nightly changelist and GitHub release notes from the latest timestamped nightly tag. |

## Cross-cutting conventions (baked into every skill)

- **Use the production binary**: resolve `LUNCOSIM_BIN` to the installed
  `luncosim` command or its absolute GitHub-installed path. In a source
  checkout without an installed command, build the production binary and use
  the resulting checkout executable (usually `target/debug/luncosim`). Do not substitute `cargo run` or an
  old `sandbox` executable for the production binary.
- **Set the executable variables once per shell session** before copying a
  command from a skill:
  `export LUNCOSIM_BIN="${LUNCOSIM_BIN:-luncosim}"` and, for a headless server,
  `export LUNCOSIM_SERVER_BIN="${LUNCOSIM_SERVER_BIN:-luncosim-server}"`.
  For the Modelica workbench use
  `export LUNICA_BIN="${LUNICA_BIN:-lunica}"`.
  Override either with the absolute path to an installed GitHub build or to a
  freshly built checkout binary when the command is not on `PATH`.
- **Always launch luncosim with its HTTP API**: `"$LUNCOSIM_BIN" --api 4101` (use
  another explicit free port when needed). Every controllable, visual, realtime,
  or scene-test luncosim process must carry
  an explicit `--api PORT`. Only parse-only `--validate` invocations are exempt.
- **Exit the previous session before launching the next**: send the API `Exit` command,
  verify the process and port are gone, then start the replacement. Never overlap luncosim
  GUI/API sessions or reuse a port while the old session is still alive.
- **curl-first** over the `mcp__lunco__*` tools; drive the app over `POST /api/commands`.
- **Discover, don't hardcode** the command set — `DiscoverSchema` enumerates it live.
- **Discover before declaring a gap**: search the relevant skills and docs first,
  then the current owner source, registrations/callers, maintained dependencies,
  and runtime/API surface. Use
  [**capability-discovery**](capability-discovery/SKILL.md) and report bounded
  evidence as found, unwired, elsewhere/version-mismatched, not found in scope,
  or externally blocked.
- **Policy → rhai, identity → USD, math → Modelica** — keep logic out of the Rust core.
- **Tutorial tests → Rhai** — put lesson-specific runtime assertions in
  `assets/scenarios/tests/*.rhai` and run them through production
  `luncosim test`; keep Rust tests generic to the scripting/lifecycle seam so
  editing a tutorial does not require rebuilding the core.
- **Rust tests → the owning target** — use
  `scripts/run_rust_tests.sh -p <package> --module <source-module>` (or
  `--filter <module>::<test>`). It selects the large crate's single
  owning crate's direct `--test <source-module>` target, uses `sccache` when
  installed, and accepts `--check` for compile-only feedback, `--no-run` to
  build without running, or `--lib` for inline library tests. Select the owning
  crate instead of invoking every workspace test target.
- **Tutorial world/time contract → USD** — choose fixed `DistantLight`, explicit
  ephemeris (`LunCoEpochAPI` plus authored `lunco:time:epochJd`), an existing
  world, or no payload. Do not leave orbital time implicit; the authored
  `epoch-api-missing-time` lint catches an epoch API without its field. See
  [`build-usd-scene`](build-usd-scene/SKILL.md) and
  [`assets/tutorials/README.md`](../assets/tutorials/README.md).
- **USD is the source of truth; the ECS is a projection of it.** An edit that
  doesn't lower to a `UsdOp` escapes save, journal, undo *and* replication —
  silently. See [**usd-projection**](usd-projection/SKILL.md).
- **Use the API `Exit`**, never `pkill`, to stop a running app.
- **Validate before you run.** `"$LUNCOSIM_BIN" --validate <files…>` parses assets in
  seconds with no GPU and catches broken references, missing wheel attrs,
  SysML/KerML diagnostics and
  `if`/`when` in Modelica — **and runs the authored lint rules**, which is what
  reports a part that would fall off a vehicle. On a *loaded* scene use the verb:
  `cmd("RunLint", #{})` + `query("LintReport")`, or `query("ValidateTwin", #{path: "..."})` for a Twin-wide namespace pre-flight; nothing lints on its own. Rules
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

New to the codebase? Start with [**repo-map**](repo-map/SKILL.md), then use
[**capability-discovery**](capability-discovery/SKILL.md) for any unfamiliar
feature. The [Documentation Hub](../docs/README.md) and the
[AI Agent Guide](../AGENTS.md) are the governing indexes and contract.

## Routing boundaries

Choose one primary skill for the requested outcome and load a neighboring skill
only when its contract is needed:

| Request | Primary | Defer to |
|---|---|---|
| Write a reusable USD asset | `author-usd-component` | `author-usd-physics` for detailed physics; `validate-assets` for pre-flight |
| Assemble existing assets into a scene | `build-usd-scene` | `build-vehicle` for mobility assemblies; `author-scenario` for behavior |
| Build a vehicle | `build-vehicle` | `authoring-vessel-controllers` for GNC; `compose-multidomain-twin` for the complete Twin |
| Create or modify a reusable assembly interactively | `edit-usd-assembly` | `usd-projection` for projection internals; `author-usd-physics` for detailed physics; `lunco-ui` for panel implementation |
| Build an AI-readable assembly or scene recipe | `author-rhai-tool` | `edit-usd-assembly` for live Editor review; `validate-assets` for pre-flight |
| Add or diagnose a controller | `authoring-vessel-controllers` | `run-modelica` for standalone Modelica execution |
| Add or diagnose USD-to-ECS machinery | `usd-projection` | `luncosim-architecture` for cross-domain ownership |
| Add or locate an asset | `use-asset-library` | `validate-assets` for file checks |
| Run a Modelica model | `run-modelica` | `test-via-api` for generic end-to-end evidence; `inspect-simulation` for read-only observation |
| Observe live state | `inspect-simulation` | `test-via-api` only when commands or verdicts are required |
| Build a Twin-wide mission | `compose-multidomain-twin` | `author-scenario` for mission policy and `author-tutorial` for lessons |
| Author or verify SysML requirements | `sysml-requirements` | `validate-assets` for parse-only gates; `compose-multidomain-twin` for complete Twin composition; `test-via-api` for generic runtime evidence |
| Workbench UI versus Twin UI | `lunco-ui` | `lunco-theme` for tokens; `runtime-ui` for authored Twin-facing surfaces |

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
Claude Code. Other hosts may use the same files directly; host-specific
metadata is optional and must not be required for the runbook to work. Keep
the cross-host entry point in [`START-HERE.md`](START-HERE.md) current when
adding or changing routing.
