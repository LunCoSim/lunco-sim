---
name: validate-assets
description: >
  How to PRE-FLIGHT a LunCoSim asset file — "does this `.mo`/`.usda`/`.wgsl`/`.rhai`
  actually parse?" — with `ValidateAsset`, before loading a scene, starting a
  cosim, or handing the file to a reviewer.
  USE THIS SKILL when the user says "validate this asset", "check the file
  compiles", "did I break the rover", "why won't this wheel spawn", "lint my
  Modelica model", "check the shader params", "syntax-check the scenario", or
  when you are about to launch the whole sandbox just to find out whether a file
  you edited is well-formed. Also use it as the CHEAP first step of any
  authoring loop (author-usd-component, build-vehicle, use-asset-library).
  Project-specific and non-obvious: it is a QUERY that returns data (not a
  fire-and-forget command), it is answered by SANDBOX binaries only (lunica does
  not link it), the CLI form starts no app/window/GPU at all, `.mo` treats
  `if`/`when` in an equation section as ERRORS (rumoca is branch-free), `.wgsl`
  can never fail (only warn), `twin://` cannot be resolved, and a bare relative
  path is tried against the CWD *before* the assets root.
---

# Validate an asset (pre-flight)

`ValidateAsset` answers one question — **does this file parse, and would the
engine accept it?** — without a scene, a cosim, a GPU, or a window. It is the
cheapest possible check and it is safe to run against a live sandbox
**mid-simulation**: it only reads files.

Implementation: [`crates/lunco-scene-commands/src/validate.rs`](../../crates/lunco-scene-commands/src/validate.rs).
Related: [`author-usd-component`](../author-usd-component/SKILL.md) (author the
file), [`use-asset-library`](../use-asset-library/SKILL.md) (get it discovered),
[`build-vehicle`](../build-vehicle/SKILL.md) (wheels), [`test-via-api`](../test-via-api/SKILL.md)
(drive the running app once it validates).

## Two invocation forms

### CLI — no app, no window, no GPU

```bash
cargo run -p lunco-sandbox --bin sandbox -- --validate \
  assets/models/RoverBattery.mo \
  assets/vessels/rovers/skid_rover.usda \
  assets/shaders/rover_hull.wgsl \
  assets/scenarios/rover_battery.rhai
```

The flag is intercepted in `crates/lunco-sandbox/src/bin/sandbox.rs:19-33`
**before** the Bevy `App` is built, and the process `exit`s — nothing is
rendered, no window opens, no port is bound. Run it anywhere, any time.

| Exit code | Meaning |
|---|---|
| **0** | every report `ok` |
| **1** | at least one report failed |
| **2** | `--validate` given with no paths |

- **Multiple paths**: everything after `--validate` up to the first argument
  starting with `--`.
- **Exact flag match only** — `--validate=path` and `-v` are not parsed.
- Output per file: `OK  <path> (<kind>)` / `FAIL  <path> (<kind>)`, then
  indented `error:` and `warning:` lines on stdout.

### API — against a running sandbox

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"ValidateAsset","params":{"path":"lunco://models/RoverBattery.mo"}}'
```

Only one param: **`path`** (string). It is a **query provider**, so the data
comes back in the response body — you do **not** poll `QueryCommandResult`.

**Answered by sandbox binaries only.** `ValidateAsset` is registered in
`SpawnCommandPlugin` (`crates/lunco-scene-commands/src/commands.rs:2923`), which
`lunica` does not link — asking lunica gives `CommandNotFound`. Use the CLI form
when only lunica is up.

## The report

```json
{"path":"…", "kind":"modelica|usd|wgsl|rhai|unknown",
 "ok":true, "errors":[], "warnings":[], "info":{}}
```

`ok == errors.is_empty()`. **Warnings never fail a file.** `path` echoes what you
passed, *not* the resolved disk path — if you need to know which file was read,
pass an unambiguous one.

## What each extension actually checks

| Ext | Checks | Can it FAIL? |
|---|---|---|
| `.mo` | rumoca `parse_to_syntax` + **branch-free lint** | yes |
| `.usda` | layer parse → **compose the reference closure** → strict `WheelParams::read` on every `PhysxVehicleWheelAPI` prim | yes |
| `.wgsl` | `ParamSchema::parse` — reflect the `struct Material` uniform + `//!@` annotations | **no** — warnings only |
| `.rhai` | `rhai::Engine::new().compile()`, nothing executed | yes |
| anything else | `unsupported extension` error | yes |

Extension gate is literal: **`.usda` only** — `.usd` and `.usdc` are rejected as
unsupported, not parsed.

### `.mo` — the branch-free lint is the point

rumoca's solver path is branch-free, so `validate.rs:207` scans the source (after
stripping comments) and emits **errors**, not warnings:

- `when` / `elsewhen` — an error **anywhere in the file**.
- `if` — an error **only inside** an `equation` / `initial equation` /
  `algorithm` / `initial algorithm` section. An `if` in a binding or a modifier
  is fine.

Fix by rewriting as `der(x) = expr` with `max()`/`min()` clamps — that is exactly
what `assets/models/RoverBattery.mo` does for its state-of-charge cutoff.

`info` carries `{model, params, inputs, outputs:null}`. `outputs` is always
`null` — outputs are not knowable before a compile.

> **Lint caveats (real false positives):** the scanner does not strip string
> literals, so a `when`/`if` inside a description string or `annotation(...)`
> is flagged. And `end if;` / `end when;` resets the "in an equation section"
> flag, so `if`s after a nested block close stop being flagged.

### `.usda` — this is the one that catches broken references

Three stages, first failure short-circuits:

1. `usda_to_data` — this file's own syntax.
2. `compose_file_to_stage` — **fetches the whole layer closure**
   (`subLayers` + `references` + `payload`, including arcs inside variant
   blocks). A dangling `@lunco://…@` is a hard error here. This is the single
   best reason to run it: [bare paths silently no-load at runtime](../use-asset-library/SKILL.md#the-lunco-scheme),
   but a *missing* target fails loudly right here.
3. `WheelParams::read` on every prim with `PhysxVehicleWheelAPI` — the **same
   strict reader the spawner uses**. The error names every missing attribute:
   `wheel /Rover/Wheel_FL would refuse to spawn — missing required attributes: …`

`info.wheel_prims` lists each wheel with `ok` and, when failing, `missing`.

> Two things it does **not** catch: binary leaf references (`.glb`/`.obj`/`.stl`
> are not layers, so a broken mesh path passes), and suspension-inherited wheel
> attrs — the reader is called with no attachment suspension, so a wheel that
> only validates once its suspension arc composes at spawn time is judged
> without it.

### `.wgsl` — cannot fail, read the warnings

There is **no naga validation** — deliberately. A syntactically broken shader
that still contains a parsable `struct Material` reports `ok: true`. What you get
is the reflected param schema (`info.shader_params` with `name`/`type`/`offset`/
`ui`/`default`, plus `uniform_size`) and two possible warnings:

- `no reflectable Material struct` — the shader exposes no tunable params and
  cannot be driven by `SetObjectProperty`.
- `not prop-pickable: engine fields beyond sun_vis` — it uses `//!@engine`
  params only the terrain pipeline fills, so the prop-material picker skips it.
  It still works as a scene shader. See
  [`use-asset-library` § Shaders](../use-asset-library/SKILL.md#add-a-shader-wgsl).

## Path resolution — the trap

`resolve()` (`validate.rs:86`) tries, in order:

1. **`Path::new(ref).is_file()`** — absolute, or **relative to the current
   working directory**.
2. `lunco_assets::engine_asset_local_path(ref)` — the `lunco://` root, itself
   `cwd`-joined.

Consequences:

- ❌ `models/X.mo` is ambiguous: it resolves to `<cwd>/models/X.mo` if that
  exists, **shadowing** `<cwd>/assets/models/X.mo`.
- ❌ Running from a subdirectory silently changes what `lunco://` means.
- ✅ Run from the repo root and pass either `assets/models/X.mo` (unambiguous
  filesystem) or `lunco://models/X.mo` (unambiguous scheme).
- ❌ **`twin://` cannot be resolved at all**, even with an instance running —
  the resolver only knows the engine root. Pass the twin file's real filesystem
  path instead.

## Where it fits

```
edit .usda / .mo / .wgsl / .rhai
        ↓
--validate            ← seconds, no GPU. Catches: syntax, broken refs, missing
        ↓               wheel attrs, if/when in Modelica, unparsable rhai.
load the scene        ← test-via-api
        ↓
drive it / assert     ← author-scenario, drivetrain_parity
```

Validate **every file you touched** before you launch anything. A `--validate`
run costs seconds; a sandbox launch that dies on a typo costs a compile.

## Anti-patterns

- ❌ Launching the full sandbox to find out whether a file parses — that is what
  `--validate` is for.
- ❌ Sending `ValidateAsset` to **lunica** and concluding the command doesn't
  exist. It is sandbox-only; use the CLI.
- ❌ Treating a `.wgsl` `ok: true` as "the shader compiles" — no naga runs.
  Only a real load proves the pipeline builds.
- ❌ Passing a bare `models/X.mo` from an arbitrary CWD and trusting which file
  was read — `path` in the report echoes your input, not the resolved path.
- ❌ Passing a `twin://` address — unresolvable; use the filesystem path.
- ❌ Reading `ok` and ignoring `warnings` on a `.wgsl` — that file can never
  report `ok: false`, so the warnings ARE the result.
- ❌ Adding an `if` to a `.mo` equation section to "handle a case" — rewrite it
  branch-free; the lint is enforcing a real solver constraint, not a style rule.
