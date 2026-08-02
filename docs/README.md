# LunCoSim Documentation

The authoritative home for LunCoSim architecture, design, and reference docs.

**Looking for something specific?** Jump to [I want to…](#i-want-to) below.
**New to the project?** Follow the [reading order](#reading-order-for-newcomers).
**An AI agent?** Read [`../AGENTS.md`](../AGENTS.md), then pick a [skill](../skills/README.md).

---

## I want to…

| … | Go to |
|---|---|
| Run the thing | [`apps/README.md`](apps/README.md) — every binary, its flags, when to use it |
| Find which crate does X | [`crates-index.md`](crates-index.md) |
| Find a Modelica model or USD part | [`component-index.md`](component-index.md) |
| Call the app from code / curl / MCP | [`commands-reference.md`](commands-reference.md) — every `#[Command]`, generated from source |
| Build a mission end-to-end | [`tutorials/`](tutorials/README.md) |
| Write rhai behaviour | [`scripting-guide.md`](scripting-guide.md) · [`behaviour-trees.md`](behaviour-trees.md) |
| Build a rover or lander | [`architecture/55-building-vessels-rovers-and-landers.md`](architecture/55-building-vessels-rovers-and-landers.md) |
| Build a Twin-facing HTML/CSS surface | [`architecture/runtime-authored-ui.md`](architecture/runtime-authored-ui.md) · [`../skills/runtime-ui/SKILL.md`](../skills/runtime-ui/SKILL.md) |
| Record a video / frame-exact capture | [`offline-recording.md`](offline-recording.md) |
| Understand how it all fits together | [`architecture/README.md`](architecture/README.md) |
| Know the rules before writing code | [`principles.md`](principles.md) · [`../AGENTS.md`](../AGENTS.md) |
| Do a task with an agent | [`../skills/README.md`](../skills/README.md) |
| Check what's specced vs built | [`../specs/README.md`](../specs/README.md) |
| Know what's currently broken | [`reviews/`](reviews/) |
| Track nightly changes | [`releases/`](releases/README.md) |

## Applications

Every runnable binary is indexed in [`apps/README.md`](apps/README.md). The
primary four:

| App | Purpose |
|---|---|
| [**luncosim**](apps/luncosim/README.md) | LunCoSim ground mobility and physics application (+ headless [server](apps/luncosim/OPS.md)) |
| [**lunica**](apps/lunica/README.md) | Modelica engineering workbench |
| [**assets-manager**](apps/assets-manager/README.md) | Download and process workspace assets |

> `cargo run` alone is ambiguous — pick a target. See the apps index.

## Skills (agents & contributors)

Task-oriented runbooks in [`../skills/`](../skills/README.md). Each triggers on a
kind of request and distils these docs into a recipe plus the project-specific
traps. Point an agent — or yourself — at one before a hands-on task.

The full catalogue with trigger phrases is in
[`../skills/README.md`](../skills/README.md). The most-used entry points:

| Skill | Use it when you want to… |
|---|---|
| [repo-map](../skills/repo-map/SKILL.md) | Get oriented — layout, which binary to run, where a feature lives |
| [build-usd-scene](../skills/build-usd-scene/SKILL.md) | Author/edit the 3D world — load, spawn, place, tune |
| [author-scenario](../skills/author-scenario/SKILL.md) | Write rhai behaviour — missions, waypoints, reactions |
| [build-vehicle](../skills/build-vehicle/SKILL.md) | Build a rover from the mobility component library |
| [authoring-vessel-controllers](../skills/authoring-vessel-controllers/SKILL.md) | Give a vessel a GNC / autopilot with manual handoff |
| [compose-multidomain-twin](../skills/compose-multidomain-twin/SKILL.md) | Assemble USD + Modelica + cosim + rhai into a Twin |
| [inspect-simulation](../skills/inspect-simulation/SKILL.md) | Observe a running sim — ports, variables, screenshots |
| [test-via-api](../skills/test-via-api/SKILL.md) | Verify a change without asking a human to click |
| [runtime-ui](../skills/runtime-ui/SKILL.md) | Author reloadable Twin-facing HTML/CSS-like runtime surfaces |

## Documentation map

| Path | What lives there |
|---|---|
| [`architecture/`](architecture/README.md) | The design narrative — how and why LunCoSim is shaped this way |
| [`apps/`](apps/README.md) | Per-binary guides: flags, controls, workflows |
| [`tutorials/`](tutorials/README.md) | Build-something-real walkthroughs (and how the in-app lessons work) |
| [`reviews/`](reviews/) | Standing known issues (`open-*.md`) and dated audit reports |
| [`releases/`](releases/README.md) | Commit-derived nightly changelists and release-note snapshots |
| [`numeric-experiments/`](numeric-experiments/README.md) | Solver/numerics investigations with reproducible setups |
| [`architecture/research/`](architecture/research/) | Prior art, inspiration, roads not taken |
| [`../specs/`](../specs/README.md) | Feature contracts, with an Implemented/Partial/Not-built index |
| `../crates/<crate>/README.md` | Per-crate quick-start — "how do I use this crate now" |
| [`../scripts/perf/README.md`](../scripts/perf/README.md) | Performance profiling subsystem |

## Reading order for newcomers

1. [`architecture/00-overview.md`](architecture/00-overview.md) — what LunCoSim is, the three-tier model, crate layers
2. [`principles.md`](principles.md) — how we work (TDD, plugin-first, interop, the documentation mandate)
3. [`architecture/01-ontology.md`](architecture/01-ontology.md) — vocabulary: Space System, Port, Connection, Typed Command
4. [`architecture/10-document-system.md`](architecture/10-document-system.md) — the data model: Documents, DocumentOps, DocumentViews
5. [`architecture/21-domain-usd.md`](architecture/21-domain-usd.md) — USD is the source of truth; ECS is a projection of it
6. [`architecture/12-api.md`](architecture/12-api.md) — the transport-agnostic command/query layer
7. [`architecture/13-twin-and-workflow.md`](architecture/13-twin-and-workflow.md) — what a Twin is; save, load, workflow
8. [`architecture/11-workbench.md`](architecture/11-workbench.md) — UI/UX: workspaces, panels, command palette
9. Domain docs as needed: `20-domain-modelica`, `22-domain-cosim`, `23-domain-environment`

## Conventions

**Numbering.** Architecture docs carry a numeric prefix; un-numbered files are
topic docs and substrates. See [`architecture/README.md`](architecture/README.md)
for the live index.

| Range | Category |
|---|---|
| `00`–`09` | Foundation — overview, ontology |
| `10`–`19` | Framework — documents, workbench, API, twin, time, simulation layers |
| `20`–`29` | Domains — Modelica, USD, cosim, environment, SysML, experiments |
| `30`–`39` | Platform — wasm/web, networking, spacecraft & multi-domain composition |
| `40`–`49` | Cross-cutting — asset I/O, axes & units, frame discipline, views, connectivity |
| `50`–`69` | Feature subsystems — visuals, camera, suspension, terrain georeferencing, lifecycle |
| un-numbered | Substrates (`*-substrate.md`) and standalone topic docs |

**Status header.** Every architecture doc opens with one line:

```
> Status: Active | Design | Draft · Audience: <who should read this>
```

- **Active** — describes the system as built. The default.
- **Design** — agreed shape, not fully built. Say what is missing.
- **Draft** — under live review; may be wrong.

**Where a doc belongs.** One topic, one home — link, never duplicate.

| Kind | Home | Answers |
|---|---|---|
| Crate README | `crates/<crate>/README.md` | "How do I use this crate right now?" |
| App README | `docs/apps/<app>/` | "What is this binary and how do I run it?" |
| Architecture doc | `docs/architecture/` | "How does LunCoSim fit together, and why?" |
| Spec | `specs/<nnn>-<name>/` | "What MUST this feature do?" (written before code) |
| Skill | `skills/<name>/SKILL.md` | "Walk me through doing this task." |

**Lifecycle.** A doc that describes shipped work stays Active — it is reference,
not history. A doc whose only content is *how we got here* (migration plans,
completed execution checklists, closed audits) is **deleted**; git remembers it.
Move a doc to `architecture/research/` only when the idea itself is worth
keeping but the path was not taken.
