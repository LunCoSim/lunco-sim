# LunCoSim — full code review, 2026-07-12

**Scope:** whole workspace (61 crates, ~233k LOC) at branch `optimization` @ `f72ad859`, plus a
line-by-line review of the branch diff vs `main` (84 files, +5155/−551).
**Method:** 8 parallel reviewers. Every finding below was verified by reading the cited code or by
running the cited command. Findings marked `PLAUSIBLE` are suspected but not proven — verify before
acting. Everything else is `CONFIRMED`.
**Benchmark:** judged against practice in simulation / planetary-scale engineering (SPICE-NAIF frame
& time conventions, IAU-WGCCRE rotation models, FMI 2.0/3.0 co-simulation, CDLOD/chunked-LOD
planetary rendering, server-authoritative + rollback netcode).

---

## ⚠️ STATUS — 2026-07-13: most of this is FIXED. Read this box first.

This document is the **original review**, kept as written for the record. It is no longer a to-do list.

| | |
|---|---|
| **§1 Security** | **DEFERRED BY DESIGN.** The project has accepted that it does not enforce access control. See [`TODO-rbac-not-enforced.md`](TODO-rbac-not-enforced.md). The one exception — the `SpawnDemTerrain.target_res` OOM (input validation, not authorization) — is **fixed**. |
| **§2 Architecture** | Fixed: `A1` (+ the avian `debug-plugin` leak it exposed), `A2`, `A3`, `A4`, `A5`, `A7`(partial), `A8`, `A10`(partial), `A11`. Open: `A6`, `A9`. |
| **§3 Simulation** | Fixed: `P9`, `P10`, `P11`. **Deliberately untouched at the user's direction: `P1`–`P6` (celestial/orbital).** `P2` in particular — **the Moon's near side still does not face Earth.** Open: `P7`, `P8`, `P12`. |
| **§4 Netcode** | **Untouched at the user's direction.** `N1`–`N6` all open. `N1` is the one users will hit. |
| **§5 Performance** | Fixed: `R1`(partial), `R2`, `R3`, `R4`, `R5`, `R6`, `R7`, `R8`, `R9`, `R10`, `R11`, `R12`, `R13`(most). |
| **§6 Branch diff** | Fixed: `D1`, `D2`, `D3`, `D4`, `D5`, `D6`, `D7`, `D8`, `D9`. |
| **§7 Hygiene** | Fixed: `H1` (clippy now green workspace-wide — see below), `H2`(partial), `H3`, `H4`, `H5`, `H7`, `H8`, `H9`, `H10`, `H11`(documented, not deleted), `H12`. Open: `H6`, `H13`. |

**Three corrections to this document, found while fixing it.** Do not trust these paragraphs:

- **`D2`** claims the hazard is baked into `surface_tex.a`. **It is not, any more** — `pack_surface_rgba8`
  writes `A = 255` and hazard is deliberately a *view*. Sampling it would have shaded the world red. The
  fix uses the baked **normal** map instead.
- **`D4`** claims the shaders hard-code the hazard palette. **Half-true** — both shaders already import a
  shared `lunco::transfer` WGSL module; only the Rust↔WGSL constant pair remains duplicated. And
  `TransferFn::SlopeHazard` **does** have a consumer (the Inspector legend), contrary to the finding.
- **`H1`'s framing was wrong**, and the right diagnosis matters more than the fix. The wasm-portability
  bans were being enforced **on native — the one target where they cannot be true.** `Instant::now`
  produced **73 false positives and zero true positives** (on native, `web_time::Instant` *is*
  `std::time::Instant` — same DefId — so clippy flagged every *correct* caller). A lint that is wrong
  every time it fires is a lint people silence; that is *why* the hole stayed open. Those bans now run on
  `--target wasm32-unknown-unknown`, where `cfg` strips native-only code and the types are distinct.

**Two bugs the new wasm gate caught immediately, which no native build or lint can see:**
the **web build was broken** (`transports/wasm.rs` never got a field added to `ApiResponseEnvelope`), and
`indexer.rs` shipped `std::time::Instant` into the browser, where `now()` **panics**.

**Also done, beyond this review:** the [render decoupling](../architecture/render-decoupling.md) — the
`--no-ui` server now links **no wgpu, no bevy_render, no bevy_pbr, no egui, no winit**.

---

## How to use this document

- Each finding has a stable ID (`S1`, `A3`, `R7`, `D6`, `H1`, `P2`, `N1`). Reference it in commits.
- Each has: **file:line → defect → concrete failure → fix**. The fix is a sketch, not a patch; read
  the surrounding code first.
- **§Fix order** at the bottom is the recommended sequence. The first nine items are one-liners.
- **§Appendix** has the exact repro commands used, so you can re-verify before and after.
- **Do not trust the docs cited in `H8`** — several assert "implemented" for code that does not exist.

---

## Executive summary

The **engineering craft is high; the enforcement is absent.** Nearly every subsystem contains a
correct, well-reasoned mechanism sitting next to a place where that mechanism is bypassed — and
nothing in the build catches it:

| | |
|---|---|
| clippy | **has never run** on the 5 biggest crates (it errors out on a dependency first) — `H1` |
| headless builds | a **missing `default-features = false`** links egui + wgpu + winit into all of them — `A1` |
| the wire | **any connected peer can run any of 212 reflected commands** on the host — `S1` |
| USD authority | the gizmo — the primary edit path — **never writes USD**; the correct path exists and is dead — `A2` |
| co-simulation | the Modelica↔Avian coupling has **no macro-step contract**; model time depends on frame rate — `A3` |
| the Moon | **W₀ is missing from the rotation model — the near side does not face Earth** — `P2` |
| docs | three assert "Status: implemented" for types/endpoints that **grep to zero hits** — `H8` |

The three remotely-exploitable bugs are `S1`–`S3`. The single highest-leverage character in the
repository is a comma (`A1`).

---

# 1. SECURITY — remote-reachable

> **DEFERRED BY DESIGN — do not "fix" these without asking.** As of 2026-07-12 the project has
> accepted that **LunCoSim does not enforce access control**: every peer on the wire and every
> process on the local API is trusted. That makes `S1`, `S2`, `S4`, `S5`, `S7` and the path-validation
> half of `S6` known, accepted gaps rather than open bugs. They are recorded, with the operating
> assumption and the work it would take to close them, in
> [`TODO-rbac-not-enforced.md`](TODO-rbac-not-enforced.md). The consequence to respect: **never expose
> a host to an untrusted network.**
>
> The one exception, now **FIXED**, is the `S6` OOM (`SpawnDemTerrain.target_res` unclamped) — that is
> input validation, not access control, and it is reachable by accident.
>
> Everything in §2–§7 below has been fixed except where noted.

### S1 · CRITICAL · Any connected peer can execute any of 212 reflected commands on the host
**`crates/lunco-networking/src/sync.rs:875-975`** (`apply_sync_command`)

Inbound envelopes resolve `type_name` against the **whole `AppTypeRegistry`**
(`get_with_short_type_path` → `ReflectEvent::trigger`). The `SyncChannelRegistry` allowlist
(`crates/lunco-networking/src/shared.rs:173-177` — only `SetPorts`, `PossessVessel`, `ReleaseVessel`,
`UpdateProfile`, `SpawnEntity`) is consulted **only on the send side** (`sync.rs:801-861`).

Authorization is `authorize()` (`crates/lunco-core/src/session.rs:811-859`), whose default is
`CommandPolicy::OPEN` for anything unregistered — `session.rs:757-768` registers only 3 capabilities,
and their own test `unregistered_command_is_open_by_default` (`session.rs:1139`) asserts the default.
Every connecting peer is auto-inserted `authenticated: true`, `Observer`, with a server token
(`crates/lunco-networking/src/server.rs:596-602`), clearing the floor immediately.

There are **188 `#[Command]` sites / 212 registered command types**. Working payloads:

```jsonc
{ "type_name": "Exit",            "data": "{\"force\":true}" }   // host process exits
{ "type_name": "SetShaderSource", "data": "{...}" }              // host filesystem WRITE
{ "type_name": "DeleteShader",    "data": "{...}" }              // host filesystem DELETE
{ "type_name": "ApplyUsdOp" | "LoadScene" | "RunScenario", ... } // arbitrary twin mutation
```
(`Exit` → `crates/lunco-modelica/src/ui/commands/util.rs:21`; shader FS commands →
`crates/lunco-sandbox-edit/src/commands.rs:3079,3314,3474`; `ApplyUsdOp` →
`crates/lunco-usd/src/commands.rs:551`.)

The wire exposes the same surface as the loopback-only HTTP admin API — to remote peers.

**Fix:** in `apply_sync_command`, reject any `type_name` whose `SyncChannelRegistry` entry is missing
or `Local`. The routing registry already *is* the intended wire surface — make it the gate. Separately
flip `CommandPolicyRegistry` to deny-by-default **for wire-origin commands** and register the intended
set explicitly.

---

### S2 · CRITICAL · Netcode private key is all zeros
**`crates/lunco-networking/src/shared.rs:13-14`**
```rust
pub(crate) const PROTOCOL_ID: u64 = 0x004C_554E_434F_0001;
pub(crate) const PRIVATE_KEY: [u8; 32] = [0u8; 32];
```
Netcode connect-token authentication is therefore nil — anyone can mint a valid token for any public
host. The comment says "Localhost MVP", but `setup_host` binds `0.0.0.0`
(`crates/lunco-networking/src/server.rs:339`) and the crate ships a production certbot path.
Combined with `S1`: **unauthenticated remote code-equivalent execution.**

**Fix:** load the key from env/keyfile. Refuse to bind a non-loopback address while the dev key is in
use (fail loud at startup).

---

### S3 · CRITICAL · A one-line rhai script hangs the entire simulator (no op budget on hooks)
**`crates/lunco-hooks-rhai/src/lib.rs:36-42`**
```rust
let engine = Engine::new();                  // ← NO set_max_operations
let ast = engine.compile(source)?;
engine.run_ast_with_scope(&mut scope, &ast)  // ← top-level RUNS at registration
```
`invoke` (`:47`) also runs on this uncapped engine. `register_hook` is exposed to every script
(`crates/lunco-scripting/src/world_bridge.rs:384`), and `RunRhai` is a public API command:

```
POST /api/commands
{"command":"RunRhai","params":{"code":
  "register_hook(\"comms.link.connected\",\"f\",\"let i=0; while true { i+=1; }\")"}}
```
The body runs during `run_ast_with_scope` inside `drain_world_scripts` — an **exclusive `&mut World`
system** (`world_bridge.rs:1452`). The Bevy main loop never returns. Hard hang, no watchdog.

Even a well-formed hook body hangs later: hooks are invoked from `lunco-core`'s authorize gate and the
journal merge path, so an uncapped `invoke` there stalls **every command**.

**Fix:** one shared `lunco_scripting::sandboxed_engine()` constructor used by **all five**
`Engine::new()` sites. Today three duplicate the caps and two omit them entirely:

| site | `set_max_operations` | `set_max_expr_depths` |
|---|---|---|
| `crates/lunco-scripting/src/world_bridge.rs:306-316` | ✅ 1M | ✅ 128/128 |
| `crates/lunco-scripting/src/backend.rs:48` | ✅ | ✅ |
| `crates/lunco-scripting/src/catalog.rs:120` | ✅ | ✅ 128/128 |
| `crates/lunco-tools-rhai/src/lib.rs:47,183` | ❌ | ❌ |
| `crates/lunco-hooks-rhai/src/lib.rs:36` | ❌ | ❌ |

Consequence of the omission beyond the DoS: a task-tree tool library will `ExprTooDeep` where the same
source compiles fine in a scenario.

**PLAUSIBLE, related:** the 1M-op cap in `build_world_engine` is **per-eval, not per-frame**. A
`while true {}` in an `on_tick` handler burns 1M ops *per FixedUpdate tick*, errors, and re-runs next
tick — a permanent tens-of-ms-per-tick tax rather than a hang. There is no "disable after N consecutive
failures" circuit breaker.

---

### S4 · HIGH · Scripts can write any component or resource with **no authorization check**
**`crates/lunco-scripting/src/bridge_core.rs:752`** (`set_component_field`), **`:791`**
(`set_resource_field`)

The structural verbs are gated — `add_component` (`:852`), `remove_component` (`:887`),
`despawn_entity` (`:911`) all call `enforce_script_authority(..., STRUCTURAL_MUTATE, ...)`. The field
setters call **nothing**.

A remote-launched script that is ownership-denied on `cmd("SetPorts", …)` can still do
`set(other_rover, "Transform.translation", [...])`, or `set_setting("SomeResource.field", …)` on any
reflect-registered resource.

**Fix:** route both through `enforce_script_authority` (`STRUCTURAL_MUTATE`, or a new `FIELD_MUTATE`
capability), and add an allow-list for the resources reachable via `set_setting`.

---

### S5 · HIGH · The `Operator` role is self-granted by sending a display name
**`crates/lunco-networking/src/sync.rs:2657-2684`** (`on_update_profile_rbac`)

Sending `UpdateProfile{name}` promotes `Observer → Operator`. `UpdateProfile` is a **declared wire
command** (`shared.rs:176`). So the only two capabilities that are actually tightened —
`TUTOR_STATUS` and `SHARE_PERSPECTIVE` (`crates/lunco-core/src/session.rs:764-765`) — collapse to the
Observer floor for any peer that types a name. The `AuthorityRole::satisfies` lattice is well-written
and well-tested, and enforces **nothing**.

**Fix:** role comes from server-side policy at connect. Never from a client-supplied display name.

---

### S6 · HIGH · Arbitrary filesystem write **and** read over the local API

- **Write:** `crates/lunco-api/src/executor.rs:483-498` (`CaptureScreenshot`) —
  `params.get("path").and_then(|v| v.as_str())`, no validation, no sandbox root, then
  `dyn_img.save(&path)` (`executor.rs:706`).
  `{"command":"CaptureScreenshot","params":{"save_to_file":true,"path":"/home/rod/.ssh/authorized_keys"}}`
  overwrites that file with a PNG.
- **Read:** `OpenFile` / `OpenFolder` / `OpenTwin` / `AddFolderToWorkspace`
  (`crates/lunco-doc-bevy/src/lib.rs:339`, `crates/lunco-workbench/src/file_ops.rs:91,104,120`) take an
  unvalidated `path: String`. Combined with the MCP tool `get_document_source`: **open any file on
  disk, read it back over HTTP.**
- **OOM:** `SpawnDemTerrain` (`crates/lunco-terrain-surface/src/terrain.rs:366-430`) — `target_res: u32`
  goes straight to `DemTerrainRequest.target_res as usize` with **no clamp**; `window_m: f32` only
  special-cases `0` and negatives. `target_res: 100000` ⇒ a 100k×100k vertex target. (Crater *count*
  **is** clamped to 250k at `terrain.rs:104` — the pattern exists, it just wasn't applied to the
  command's own params.) `uri: String` is likewise an unvalidated filesystem path.

The API **is** correctly bound to `127.0.0.1` (`crates/lunco-api/src/transports/mod.rs:111`) — good —
but these turn "can reach port 4101" into "owns the box". Note also `mod.rs:61` uses an
`UnboundedSender` with no rate limit; any local process can flood it.

**Fix:** confine `CaptureScreenshot.path` to a configured screenshot dir (reject absolute paths and
`..`); root-confine the `Open*` commands to the workspace/twin, or mark them `ApiVisibility::hide` so
only the in-process UI can fire them; clamp `target_res` and validate `uri` against the asset roots.

---

### S7 · MEDIUM · Journal author is trusted from the wire; spoofing **suppresses the victim's edits**
**`crates/lunco-networking/src/sync.rs:1467-1493`**

Inbound `JournalEntry` is gated on `capability::JOURNAL_EDIT`, which is **absent from the default
`CommandPolicyRegistry`** → resolves to `OPEN` (Observer floor). Any peer may append USD ops that the
host merges (`crates/lunco-networking/src/journal_plane.rs:136-141`) and fans out to every other peer.

Worse: `apply_inbound_entry` trusts `entry.id.author` **from the wire**, and
`broadcast_journal_entries` filters a client's outbound entries on `entry.id.author != me`
(`journal_plane.rs:185`). So a spoofed author **prevents the victim's own edits from ever being
relayed**. That's not cosmetic — it's a silent censorship primitive.

**Fix:** default `JOURNAL_EDIT` to `Operator`; the host rewrites/validates `entry.id.author` against
the connection-bound sender identity before `append_remote`.

---

# 2. ARCHITECTURE

### A1 · SEV-1 · One missing `default-features = false` links egui + wgpu + winit into every headless build
**`crates/lunco-workspace/Cargo.toml:14`**
```toml
lunco-doc-bevy = { path = "../lunco-doc-bevy" }   # ← lunco-doc-bevy default = ["ui"] → dep:bevy_egui
```
Eight other consumers get this right (modelica, networking, obstacle-field, sandbox, sandbox-edit,
scripting, tutorial, usd). Cargo unifies features across the graph ⇒ `ui` is ON for **everyone**.

Proven with cargo itself:
```
$ cargo tree -p lunco-sandbox-server | grep -c .      # 840 crates  (GUI build: 913)
$ cargo tree -p lunco-sandbox-server -i bevy_egui
bevy_egui ← lunco-doc-bevy ← lunco-workspace ← lunco-modelica ← …
```
The "lean" `--no-ui` server links `bevy_egui, bevy_winit, bevy_render, bevy_pbr, wgpu, naga, winit,
wayland-client, x11rb, egui`. The **wasm worker**, built `--bin lunica_worker --no-default-features`
*expressly to stay small*, still links wgpu + naga + egui + bevy_render.

`lunco-workspace`'s own package description says *"Headless and UI-free … no render/winit/egui"*, its
`bevy` dep is hand-stripped to `["bevy_log"]`, and `grep egui crates/lunco-workspace/src/` returns
**zero API calls**. The `ui` feature is 100% accidental.

**Why it survived:** nothing enforces it, and CI *rationalized the symptom* —
`.github/workflows/integration.yml:70-72` explains that the Linux windowing headers are needed *"even
for the headless `--no-ui` sandbox binary (the deps are compiled in; only the runtime window is
skipped)"*. The team saw winit compiling into the headless build and wrote it off as normal.

**Fix (one comma):**
```toml
lunco-doc-bevy = { path = "../lunco-doc-bevy", default-features = false }
```
Then add a CI guard that must **fail**:
```bash
cargo tree -p lunco-sandbox-server -i bevy_egui   # expect: "package ID not found"
```
and delete the rationalization in `integration.yml`.

**Bonus:** this single line delivers ~90% of the stated "Modelica egui-free" goal. `lunco-modelica`'s
own feature gate is already drawn correctly (`default-features = false` gives `ModelicaCorePlugin`
headless; `usd-sim` imports only `source_asset::ModelicaSource` + `ModelicaSet`; no UI leaks into
non-UI consumers — verified by `lyon_tessellation` being absent from the server tree).
`lunco-workspace` was dragging egui in *underneath* it.

Same omission at `crates/lunco-workbench/Cargo.toml:39`, but that crate is hard-egui **and** absent
from the server tree — harmless, leave it. **Also** `lunco-cosim`'s `[dev-dependencies]` take
`lunco-modelica` with default features, so `cargo test --workspace` unifies `modelica/ui` ON for the
whole graph — add `default-features = false` there too.

---

### A2 · SEV-1 · "USD is the source of truth" is not enforced on the two hottest edit paths

Stated design: USD document authoritative, ECS a projection. The projection machinery is real and good
(`crates/lunco-usd/src/twin_projection.rs:311` → `live_consume.rs:61` — one-way, op-driven, journaled).
But:

- **The gizmo never touches USD.** `crates/lunco-sandbox-edit/src/gizmo.rs` contains **zero** USD
  references (`grep -c 'usd\|Usd' → 0`). Drag writes `Transform` directly; **the move is lost on
  reload.** The file states the violation out loud at `gizmo.rs:162-178`: *"the visual editor is the
  absolute authority on `Transform`"*.
- **The correct path exists and is dead.** `persist_move_to_runtime_layer`
  (`crates/lunco-sandbox-edit/src/commands.rs:2004`, authors `UsdOp::SetTranslate`) observes
  `MoveEntity` — which is **only ever fired from tests**.
- **`SetObjectProperty` is ECS-first** (`commands.rs:2663`): mutates `Visibility`, `StandardMaterial`,
  `ShaderMaterial`, `WheelRaycast` in place. Its shadow-write `persist_property_to_runtime_layer`
  (`:2055`) is gated to **exclude** shader/visible/PBR/colors, and its own doc comment admits *"is lost
  on reload."*
- **The design doc is false.** `docs/usd-source-of-truth-ecs-projection-design.md:3` says
  **"Status: implemented"** and claims `UsdPrimIndex`, `UsdAttrProjection`,
  `project_usd_attrs_to_components` were built. **All three grep to zero hits repo-wide.**

**Bug class invited:** two sources of truth for *authored* state ⇒ edits silently lost on reload, and
networked clients replaying the journal diverge from the host's ECS.

**Fix:** fire `MoveEntity` from `restore_gizmo_dynamic` (`gizmo.rs:188`) on drag-end — the authoritative
path already exists, **zero new machinery**. Then fold `on_set_object_property` into op-authoring. Mark
the design doc as *planned*.

**Legitimate exemption, correctly done (do not "fix" this):** the avian physics writeback is a
documented, deliberate boundary — `crates/lunco-usd-avian/src/big_space_bridge.rs:1-52` severs avian's
f32 `GlobalTransform` sync wholesale and never writes solver pose to USD. Solver pose is *derived*
state, not *authored* state. `LiveRebuildExempt` suppresses re-projection, not authoring. Spawn/remove/
reference **are** USD-first (`commands.rs:2390`).

---

### A3 · SEV-1 · The co-simulation coupling has no macro-step contract; model time depends on frame rate
**`crates/lunco-modelica/src/worker.rs:1472-1533`** (`spawn_modelica_requests`), scheduled in
`FixedUpdate` at **`crates/lunco-modelica/src/lib.rs:1429-1432`**

Each fixed tick, the system sends `Step{dt}` to a **background thread** and sets `is_stepping = true`.
Nothing waits. `handle_modelica_responses` clears the flag when the result lands.

Two `FixedUpdate` iterations inside one render frame run back-to-back in microseconds, so **the second
always sees `is_stepping == true` and skips**. Net effect:

> **The Modelica model advances at most once per RENDER FRAME, while Avian and `SimTick` advance once
> per FIXED step.**

Concrete failures:
- **30 FPS, rate 1×:** world advances 60 ticks/s; Modelica advances 30 steps × 1/60 s = **0.5 s of
  model time per wall-clock second**. A battery / thruster / balloon model runs at **half speed**.
  **Change your GPU load → change the physics answer.**
- **`TimeTransport.rate = 10`:** ~10 fixed steps per frame, ~1 Modelica step per frame ⇒ **Modelica
  runs 10× slower than the world it is coupled to.**
- **No catch-up:** `dt` is always `Time<Fixed>::delta` (`worker.rs:1480`), never
  `world.sim_secs − model.current_time`. Lost macro-steps are **lost model time, permanently**.
- **Nothing measures the divergence.** `ModelicaModel.current_time` is the stepper's own clock and is
  never compared to `WorldTime::sim_secs`.

`docs/architecture/19-unified-time-and-clock.md` contradicts itself about this — `:304` claims
"physics + Modelica + epoch move together", `:57` admits "Modelica … stays 1×". The code implements
neither coherently.

Against FMI 2.0/3.0 co-simulation practice:

| FMI-CS mechanism | present? |
|---|---|
| defined communication interval (macro step) | **No** — it's whatever the worker manages |
| input extrapolation over the macro step | **No** — zero-order hold on a *stale, variable-age* value |
| step rejection / rollback (`fmi2DoStep` + `SetFMUState`) | **No** — no checkpoint, no rejection |
| documented explicit-coupling error bound | **No** — `docs/architecture/22-domain-cosim.md` never mentions coupling error, delay, or Jacobi vs Gauss-Seidel |
| algebraic-loop / stiff-coupling handling | **No** |

The *ordering within a tick* is well-specified on paper
(`crates/lunco-usd-sim/src/cosim.rs:1797-1817`):
`HandleResponses → sync_modelica_outputs → Propagate → ApplyForces → sync_modelica_inputs →
SpawnRequests(Step) → [avian FixedPostUpdate]`. But a `Step` dispatched at tick N returns at tick
**N+k**, k ≥ 1, **wall-clock dependent** — so the forces Avian integrates at tick N came from a Modelica
state at N−k with inputs from N−k−1.

**The exchange itself is clean** — `crates/lunco-cosim/src/systems/propagate.rs:146-207` is an
allocation-free, FMI-shaped read-outputs→write-inputs pass with resolved value references. **It is the
algorithm around it that is missing.**

**Fix, in order of rigor:**
1. **Gauss-Seidel with a barrier** — block the fixed step on the worker result, bounded by a deadline;
   on timeout freeze the tick rather than drift. This is what `fmi2DoStep` semantics actually imply.
2. **Macro-step to the communication point** — send
   `dt = (world.sim_secs − model.current_time).clamp(0.0, MAX)` so a late model catches up, and assert
   `|model.current_time − world.sim_secs| ≤ H` each tick, surfacing the lag as a diagnostic instead of
   silently integrating a different world.
3. Run cheap Tier-B models **inline** on the fixed step (they are slow-domain; the reason for the
   thread is *compile*, not *step*).

---

### A4 · SEV-1 · An adaptive implicit solver is stepped inside the client-predicted physics loop
**`crates/lunco-modelica/src/worker.rs:38-47`**

The **live** stepper is built from `stepper_options_from_bounds` — i.e. the **same options as the batch
runner** (`crates/lunco-modelica/src/experiments_runner.rs:1166-1169`): **BDF / diffsol adaptive
implicit**, `atol = rtol = 1e-6`. Then `worker.rs:1121-1124`:
```rust
let capped_dt = dt.min(MAX_STEP_DT);            // 0.033
let sub_dt = capped_dt / STEP_SUBSTEPS as f64;  // /3
for _ in 0..STEP_SUBSTEPS { stepper.step(sub_dt)?; }
```
An adaptive solver driven at 3 fixed stop-times per macro step. Its internal step sequence is chosen
from **per-machine floating-point error estimates** — precisely what
`docs/architecture/28-modelica-realtime-physics.md` §1 says must never enter the prediction loop.

`docs/28` says a program may drive a force on a client-predicted body only if it declares the
realtime-safe promise, and that the promise is *"declared in USD, never inferred"*. **grep finds no
promise anywhere in the code** (`crates/lunco-cosim/**`, `crates/lunco-usd/**`: zero hits). So today
**any** Modelica model can be wired to `force_y` on a client-predicted `Dynamic` chassis, and nothing
stops it. The doc's load-bearing anti-goal is unenforced.

**Fix:** implement the USD-declared `lunco:program:realtimeSafe` promise and hard-refuse a
`SimConnection` from a program that has not made it into an avian force/torque port on a predicted
body. Failing that, at minimum make the **live**
path use a fixed-step explicit / semi-implicit solver family, distinct from the batch path.

---

### A5 · SEV-2 · `Step` commands are squashed — silently discarding simulated time
**`crates/lunco-modelica/src/worker.rs:432-448`** + `is_squashable` at **`:968-975`**

Two queued `Step`s for the same entity: the earlier is **dropped** and a **fake success** (`result_ok`)
is sent back for it.

Squashing is correct for `UpdateParameters` (an idempotent setpoint) and `Compile`. It is **wrong for
`Step`, which is not a setpoint — it is an integration.** Dropping it deletes `dt` of model time and
reports success.

Latent today (the per-model `is_stepping` gate makes ≥2 in-flight `Step`s rare) — but it is a loaded
gun sitting directly under the co-sim clock, and any change to that gate fires it.

**Fix:** one line — remove `Step` from `is_squashable`. If backpressure is genuinely needed, **coalesce
by summing `dt`**, never by dropping.

---

### A6 · SEV-2 · Netcode depends on the 13.4k-LOC editor for two types
**`crates/lunco-networking/Cargo.toml`** → `lunco-sandbox-edit` (real, non-optional).

Total usage: **3 lines, 2 symbols** — `commands::PendingCorrection`
(`crates/lunco-networking/src/diagnostics.rs`) and `commands::SpawnEntity`
(`crates/lunco-networking/src/shared.rs:177`).

This single edge drags the whole editor closure (→ modelica → workspace → doc-bevy) into every
networking build, and is a load-bearing contributor to `A1`'s blast radius.

**Fix:** move `SpawnEntity` + `PendingCorrection` down into `lunco-core`. Delete the edge.

---

### A7 · SEV-2 · The Inspector hardcodes exactly what the project's flagship goal says it must derive
**`crates/lunco-sandbox-edit/src/ui/inspector.rs`** — **2261 lines, zero `bevy_reflect` / `TypeRegistry`
usage** (verified: grep count 0).

It is a fixed, hand-ordered chain of ~16 bespoke `if let Some(get::<ConcreteType>())` sections
(`inspector_content:255-580`): Transform `:349`, Physics `:407`, Wheel `:448`, PBR `:520`,
TerrainShader `:532`, Joint `:573`. Six sections (`:284-314`) are unconditional globals that render
with no selection at all. Adding a component means editing this function and writing a new
`*_section` fn — precisely what *"Inspector DERIVES params, doesn't hardcode"* forbids.

**The bitter part: the derive machinery already exists two functions away.**
`usd_parameters_section:604` derives sliders purely from USD `customData {min,max,unit}`.
`shader_parameters_section:1794` derives from a reflected WGSL `ParamSchema`. The pattern works — it
just only ever covers USD attrs and shader uniforms, never Rust components.

**Fix:** `#[derive(Reflect)] #[reflect(Component)]` on the ~10 types + drive egui from
`ReflectRef::Struct`; keep bespoke sections as `inventory`-registered overrides keyed by `TypeId`.
**Deletes ~1400 lines.** (Second inspector, same disease:
`crates/lunco-modelica/src/ui/panels/inspector.rs:108` matches on `selection_kind`.)

---

### A8 · SEV-3 · A second, divergent history mechanism
**`crates/lunco-sandbox-edit/src/undo.rs`** (151 lines) — `UndoStack` + `UndoAction::{Spawned,
TransformChanged}`, keyboard-driven (`lib.rs:76,145`), fed only from
`ui/inspector.rs:264,369,584`. In-memory; **not journaled, not networked, not persisted, no inverse
ops, no author scope.**

The canonical mechanism is `lunco-twin-journal` (2113 lines — Lamport-ordered, op+inverse,
`UndoManager`/`UndoScope`, adapters in modelica/scripting/usd/sandbox-edit, commands `UndoDocument` /
`RedoDocument` at `crates/lunco-doc-bevy/src/lib.rs:228,240`).

A 3D-editor spawn/move undone via `UndoStack` leaves the Twin journal untouched ⇒ **the two histories
disagree.** Combined with `A2` (gizmo bypasses USD), the editor has both a private *authority* and a
private *history*.

**Fix:** delete `undo.rs`; route those 3 call sites through `UndoDocument`.

---

### A9 · SEV-3 · The "universal command journal" does not journal commands
**`crates/lunco-api/src/executor.rs:114-194`** (`api_command_dispatcher`) is the one funnel every
HTTP / MCP / rhai / UI command passes through. It reflects, deserializes, triggers. It has **zero
journal interaction** — no `JournalResource`, no `record_op`, no op-id.

`lunco-twin-journal` records only `DomainKind::{Usd, Modelica, Script, Shader, Experiment,
ObstacleField, ToolLibrary, Timeline}` — i.e. **authoring-document ops**.

⇒ `SetPorts`, `SpawnEntity`, `PossessVessel`, `SetTerrainOverlay`, `SpawnDemTerrain`, `DriveRover`, all
time control — **none** are journaled, replayable, or undoable. Load a twin, drive a rover, spawn
terrain, close, reopen: the journal replays *document* state only; **the entire runtime mutation
history is gone.** Deterministic replay of a *session* is impossible today.

**Fix (if wanted):** `api_command_dispatcher` is the correct single seam — one recorder call away. Add
an opt-in `DomainKind::Command` (`op` = `{name, params}` post-globalize; `inverse` = per-command
inverse or `None` ⇒ non-undoable but replayable). `globalize_command_ids` (`executor.rs:222`) already
produces the wire-stable form. **If not wanted, stop claiming it in the docs.**

---

### A10 · SEV-3 · The dynamic-registry goal is violated exactly where it matters most

The project **has** a working dynamic-registry pattern —
`inventory::collect!(AssetSchemeProvider)` (`crates/lunco-assets/src/asset_sources.rs:41-94`), with a
link-time-collection test. It is used in **exactly one** subsystem. Meanwhile:

- **`crates/lunco-usd-bevy/src/lib.rs:696-1046`** (`instantiate_prim`) — a **~370-line if/else-if
  chain** over prim type plus ~10 `lunco:*` token probes. The prime `inventory` target: adding a prim
  behavior means editing this function.
- **`crates/lunco-usd-sim/src/lib.rs`** — USD **API schema names are a ready-made registry key**, and
  they are probed by hardcoded `has_api_schema(path, "PhysxVehicleTankDifferentialAPI")` chains
  (`:502,554,876,934,960,963`). Cleanest `inventory` win in the repo.
- **Two hand-synced tables that have already drifted:**
  `crates/lunco-sandbox-edit/src/commands.rs:2597` `wheel_param_setter` (11 arms, string→field-setter)
  and `:2131` `wheel_property_usd_attr` (8 arms, string→USD-attr) describe the same fields. The comment
  at `:2126` **concedes** that `slip_stiffness` / `friction_mu` exist in one and not the other.
  **Fix:** one `Reflect`-derived struct + `#[usd(attr = …)]`.

**Two suspected violations were false — credit where due:** panels **are** a trait-object registry
(`crates/lunco-workbench/src/lib.rs:495` — `HashMap<PanelId, Box<dyn Panel>>`, 25+ `register_panel`
sites; there is no `enum Panel`), and there is **no** hardcoded material table
(`crates/lunco-materials/src/dyn_params.rs` reflects the WGSL `struct Material` for names, offsets,
ranges). Both stated goals are *met* there.

---

### A11 · Actual bug · Duplicate match arm swallows an intent
**`crates/lunco-core/src/architecture.rs:133,142`**
```rust
133:  "backward" | "back" | "movebackward" | "pitch_up" => Some(UserIntent::MoveBackward),
142:  "cancel"   | "back" | "unpossess"                 => Some(UserIntent::Cancel),
```
`"back"` is bound twice; arm 133 wins. A scene authoring `"back"` for unpossess **silently drives
backward instead**. rustc already emits an unreachable-pattern warning here (`architecture.rs:142:20`)
and it is being ignored. Exactly the collision a registry — which errors on duplicate key insert —
makes impossible.

---

# 3. SIMULATION CORRECTNESS (vs aerospace practice)

### P1 · CONFIRMED · `WorldGrid` config disables cell binning in the primary grid
**`crates/lunco-core/src/world.rs:75`**
```rust
Self { cell_edge_length: 2000.0, switching_threshold: 1.0e10 }
```
big_space computes `maximum_distance_from_origin = edge/2 + threshold` = **1e10 m**, and
`translation_to_grid` (`big_space/src/grid/mod.rs:114`) short-circuits:
```rust
if input.abs().max_element() < self.maximum_distance_from_origin as f64 {
    return (CellCoord::default(), input.as_vec3());   // ← raw f32, cell 0
}
```
⇒ **every entity in `WorldGrid` under 1e10 m stays in cell (0,0,0) with a raw f32 translation**, and
`LocalFloatingOrigin::translation` (f32) is bounded by the same 1e10.

| distance | f32 ULP |
|---|---|
| 3.8e8 m (Earth–Moon) | **32 m** |
| 1e9 m | **64 m** |

**The codebase already knows:** `crates/lunco-core/src/coords.rs:230` — *"the live WorldGrid uses 1e10
⇒ never bins ⇒ cell always 0, which is exactly what S2 will change."* The root grid one line below
(`world.rs:143`) correctly uses `Grid::new(edge, 100.0)`.

**Fix:** `switching_threshold: 100.0`. It is a **precision** knob, not an extent knob — cells are i64,
so small edges cost nothing.

---

### P2 · CONFIRMED · No IAU rotation model — the prime-meridian epoch W₀ is simply absent
**`crates/lunco-celestial/src/geo.rs:67-74`**
```rust
let days = epoch_jd - lunco_time::J2000_JD;
let tilt = DQuat::from_rotation_arc(DVec3::Y, desc.polar_axis.normalize_or_zero());
tilt * DQuat::from_axis_angle(DVec3::Y, days * desc.rotation_rate_rad_per_day)
```
Longitude 0 is defined as *"+X of the ecliptic frame at J2000"* (module doc, line 13). The
IAU/WGCCRE definition is **`W = W₀ + Ẇ·d`**, with **W₀ = 38.3° (Moon)** and **W₀ ≈ 190.147° (Earth)**.
The *rate* is correct (0.2299708 rad/day = 13.17636 °/day ✓). **The phase is absent.**

- **Moon:** every lat/lon is rotated **38.3°** from its true inertial direction — **~1160 km of surface
  at the equator. The near side does not face Earth.** Sub-solar longitude (lunar local solar time) is
  wrong by **~2.9 days**.
- **Earth:** W₀ ≈ 190° missing ⇒ every ground station is ~190° of longitude off ⇒ DSN/comms visibility
  windows wrong by **~12.7 h**.

The existing test (`crates/lunco-celestial-ephemeris/src/lib.rs:324`) only checks Shackleton's
*elevation*, which is **longitude-insensitive at the pole** — it cannot see this.

Also: `polar_axis` is a hand-typed ecliptic-frame snapshot (*"mean-of-2026 … good to ~0.1°/yr"*,
`crates/lunco-celestial/src/registry.rs:126-134`) instead of IAU α₀/δ₀. No libration, no precession, no
nutation. The comment is honest, but the claim at `registry.rs:47` that the values are *"extracted from
the IAU WGCCRE recommendations"* is not true of the code.

**Fix:** store `pole_ra_deg`, `pole_dec_deg`, `w0_deg`, `w_rate_deg_per_day` per body (WGCCRE Table 1);
derive **both** `polar_axis` and `body_rotation` from them.

---

### P3 · CONFIRMED · Kepler elements are referenced to the ecliptic, not the body's equator
**`crates/lunco-celestial/src/kepler.rs:5-8`** claims *"referenced to the engine equator of the central
body … the body's pole = +Y — the same pole latitudes use in `geo`."*

But `geo::body_rotation` uses the **tilted** `polar_axis`, while `position_bevy_m` builds the orbit
about **+Y**, and `crates/lunco-celestial/src/placement.rs:336` cancels the full rotation:
```rust
(body_rotation(desc, jd).inverse() * p_inertial, Quat::IDENTITY)
```
`R⁻¹·p` rendered through the grid's `R` gives back `p` ⇒ inclination stays measured about the
**ecliptic** pole.

For Earth (23.44° tilt), an i = 51.6° ISS-like orbit is inclined 51.6° **to the ecliptic** ⇒
**ground-track latitude wrong by up to ±23.4°**, and RAAN is not comparable to any TLE. (Moon, 1.5°
tilt, is only mildly wrong.)

**Fix:** `let tilt = DQuat::from_rotation_arc(DVec3::Y, desc.polar_axis.normalize());` and use
`tilt * p_inertial` — then elements share `geo`'s pole, as the doc already claims.

---

### P4 · CONFIRMED · Docs and code disagree about which layer rotates; the orbit camera spins
`crates/lunco-celestial/src/big_space_setup.rs:10-21` documents: *"Grid Anchor (inertial) — does NOT
rotate … Body Entity (rotating) — rotates via `body_rotation_system`."*

`crates/lunco-celestial/src/systems.rs:114` — `Query<(&mut Transform, &CelestialReferenceFrame)>`, and
`CelestialReferenceFrame` is on the **grids** (`big_space_setup.rs:219,302,316,395`), never on the
bodies. **The grids spin; the bodies are identity.**

So the Observer Camera at `big_space_setup.rs:521` — `.set_parent_in_place(earth_grid); // On Earth
Grid (inertial) for orbit view` — is parented to a frame rotating at **1 rev/sidereal-day**. **The orbit
view is not star-fixed.**

(The rest of the code — `placement.rs:327-336`, the `coords.rs:315` test — correctly assumes rotating
grids. Only the `big_space_setup` doc block and the camera-parenting rationale are stale.)

---

### P5 · CONFIRMED · No light-time, no aberration, anywhere
```
$ grep -rn 'light_time\|aberration\|speed_of_light\|299792' crates/    # → ZERO hits
```
Ephemeris positions are geometric-at-epoch. `crates/lunco-celestial/src/comms.rs:292` publishes
`range_m` with **no `light_time_s` / `delay_s` port**.

Earth↔Moon one-way light time is **1.28 s** — the dominant constraint in any teleoperation scenario
this simulator exists to study. It is not modeled, and not documented as unmodeled.

**Fix:** `light_time_s = range_m / 299_792_458.0` as a comms port — cheap, correct, immediately useful.
Document that ephemeris positions are geometric (aberration ≈ 20.5″ is negligible for lighting — *say
so* rather than leaving it silent).

---

### P6 · CONFIRMED · Solar azimuth is 180° off relative to the codebase's own north
**`crates/lunco-environment/src/solar.rs:80`** — `let azimuth = d.x.atan2(d.z)` is zero when the sun
lies along **+Z**, which `crates/lunco-celestial/src/geo.rs:16` defines as **South** (North = −Z).

Every Modelica sun-tracker consuming `SOLAR_AZIMUTH_CONNECTOR` therefore gets a **south-referenced**
azimuth with nothing saying so. Same file: `LocalSolar` is derived from the render `DirectionalLight`
in *world* axes, which only equals site-ENU in site-anchored scenes — an unstated precondition.

---

### P7 · CONFIRMED · The USD importer ignores stage `metersPerUnit` / `upAxis`
`docs/architecture/41-axes-and-units.md` mandates *"convert once, at the importer"*. `lunco-usd-bevy`
reads **neither** (`grep metersPerUnit` hits only `terrain-surface/georef.rs` and a DEM-path warning in
`sandbox/lib.rs:2936`; `upAxis` appears only on **export**, `usd-bevy/src/lib.rs:3900`).

An Omniverse / Isaac Sim stage (Z-up, centimetres — *their* defaults) imports **rotated 90° and 100×
too small**, silently.

---

### P8 · The frame-typing decision deserves re-litigating
Every position is a bare `DVec3`; the frame lives in the variable name — `rel_pos_au`, `pos_bevy_m`,
`p_m_geo_au`, `body_local`, `cam_abs`, `site_in_solar`. At least **eight** live frames: ICRS-equatorial
AU · ecliptic AU · ecliptic-Bevy m (solar) · EMB-relative · body-fixed · site-ENU · grid-local
(cell + f32) · origin-relative render.

`specs/009-coordinate-frame-tree` explicitly chose *"discipline, not types"*
(`docs/architecture/41` §Recommendation), and its own status line admits *"named TF-tree types
absent"*. That is a defensible call — **except that the two most expensive bugs preserved in this
code's own comments are both silent frame mixes:** equatorial vectors fed to ecliptic geodesy (sun 45°
below the horizon at Shackleton — `celestial-ephemeris/src/lib.rs:222-232`) and an ecliptic sun
direction fed into the site frame (`celestial/src/systems.rs:206-216`).

Newtypes (`Icrf`, `EclipticAu`, `SolarM`, `BodyFixed<Body>`, `SiteEnu`) are **zero-cost** and would have
made both **unrepresentable**. This is the highest-leverage remaining fix in this section, and it is
*cheaper than the incident cost already paid*.

Related — **`ephemeris.rs:56-63` hardcodes the frame tree as a `match`** (`399→3, 301→3, 3→10,
-1024→399`) while `BodyDescriptor::parent_id` (`registry.rs:63`) already carries the same tree. Two
sources of truth. Unknown ids fall through to `DVec3::ZERO` (`celestial-ephemeris/src/lib.rs:299`) — a
mission body whose CSV failed to fetch **renders at the Sun's center**, indistinguishable from a valid
position.

---

### P9 · The DAC "same-tick" guarantee is an accident of scheduling
`crates/lunco-mobility/src/lib.rs:101-104` claims wheels *"read `PhysicalPort` AFTER the DAC has
propagated **this tick's** `DigitalPort` command into it"*, and `crates/lunco-core/src/lib.rs:486-500`
documents `ControlDacSet` on the fixed clock (this is the steering-jitter fix, and it is right).

But the **producers** — `drive_from_bindings` (`crates/lunco-controller/src/lib.rs:43`) and
`drive_autopilots` (`crates/lunco-autopilot/src/lib.rs:1320`) — are added to `FixedUpdate` with **no
ordering relative to `ControlDacSet`**, and they emit via `commands.trigger(SetPorts)`. The `SetPorts`
observer (`crates/lunco-cosim/src/lib.rs:253-267`) then queues **another** `world` closure to do the
actual write. **Double-deferred.**

With no dependency edge, both flush at the schedule's end-of-tick sync point — i.e. **after**
`ControlDacSet` and after the wheel systems. Real path: input at N → `DigitalPort` at end of N → DAC at
**N+1** → wheels at **N+1**. Deterministic today only as an accident of where the auto-inserted sync
points land; **add one `.after()` anywhere in that graph and the latency changes.**

**Fix:** order `drive_from_bindings` / `drive_autopilots` `.before(ControlDacSet)` explicitly, and make
`on_set_ports` write through directly (it already has world access via the observer) rather than
re-queueing.

---

### P10 · Wire summation order is ECS-iteration order (f64 rounding, cross-peer)
**`crates/lunco-cosim/src/systems/propagate.rs:90-119`** compiles wires from
`world.query::<&SimConnection>().iter()` — **archetype/table order**. Multiple wires into one input
**sum** (`:185` — `acc[w.dst_index] += src * w.scale + w.offset`) in that order.

Host and client spawn `SimConnection` entities via different paths (local USD load vs replicated
spawn) ⇒ summation order can differ ⇒ different f64 rounding ⇒ **a bit-level divergence at the root of
the force path.**

Same class as the already-fixed `children()` hash-order churn — but that fix was applied **in the USD
layer only**; the co-sim fabric was never audited for the same property.

**Fix:** sort `wires` by a stable key (`(dst_index, src GlobalEntityId, src_port)`) at compile time in
`rebuild`. One `sort_by`, **zero per-tick cost**.

---

### P11 · `MAX_REALTIME_RATE = 100` guarantees a death spiral before it ever helps
**`crates/lunco-time/src/lib.rs:47`**

Bevy clamps the **raw** delta to `max_delta` (33 ms — `crates/lunco-sandbox/src/lib.rs:1447`) and *then*
multiplies by `relative_speed`. At rate 100, a hitched frame yields 33 ms × 100 = 3.3 s of virtual time
= **198 fixed steps in one frame**, each with `SubstepCount(12)` of Avian. That frame is slow, which
re-clamps to 33 ms, which yields another 198 steps. **The clamp does not save you — it pins you at the
worst case.**

`docs/architecture/28` §3.3 explicitly asks for a *"step budget … on exceed, degrade fidelity rather
than spin"*. No such budget exists in code.

**Fix:** cap fixed steps per frame (manual accumulator drain limit) and lower `MAX_REALTIME_RATE` to
what the solver actually sustains — **measure it**; likely 4–8× on native.

---

### P12 · There is no rollback, and the spec says there is
`specs/005-multiplayer-core` FR-003: *"Server state corrections are reconciled via rollback."* Module
headers say *"input-replay model (D2)"*.

Reality: `crates/lunco-core/src/reconcile.rs:79-112` is **pure geometry** —
`Correct { pos: current_pos + err_pos * 0.3 }`. No re-simulation, no re-application of unacked inputs.
Worse, the buffered inputs are **literally zeroed**:
```rust
// crates/lunco-controller/src/lib.rs:248-254
entry.frames.push_back(InputFrame { seq, tick, forward: 0.0, steer: 0.0, brake: 0.0 });
// comment: "The forward/steer/brake payload is unused by the current positional reconcile
//           (awaits true input-replay)"
```
`OwnedInputLog` is a seq/tick ledger, not an input history. **Replay is impossible as built.**

Consequence: the correction rests on *"error at the ack ≈ error now"*, which holds in free driving and
**fails precisely in contact / transient regimes** — the divergence class the project keeps re-hitting.

**On determinism:** because there is no re-simulation, bit-determinism is *not currently required* — so
the float determinism, `HashMap` iteration in `diff_peer_batch`/`compute_interest_sets`, and
`Time`/`std::env::var` reads inside networked systems (`sync.rs:1287,2032`) are **harmless today**. But
they mean rollback **cannot be retrofitted** without a rewrite: a native x86 f64 avian host and a wasm
client will not re-simulate bit-identically regardless of ECS hygiene.

**Recommendation: delete the rollback language from the spec and docs; commit to state-sync +
smoothing.** That is the right call for a lunar sim whose cosim/Modelica forces clients can't reproduce
anyway — the `NotPredictable` marker already concedes it.

---

# 4. NETCODE BUGS (no attacker needed)

### N1 · HIGH · `AppliedInputSeq` permanently kills prediction on any re-possessed vehicle
**`crates/lunco-controller/src/lib.rs:234-239`**
```rust
let slot = applied.0.entry(g).or_insert(0);
*slot = (*slot).max(cmd.seq);   // cmd.seq comes straight off the wire
```
`AppliedInputSeq` (`crates/lunco-core/src/session.rs:631`) is keyed **by gid only**, never by owner, and
**never cleared** — not on despawn, not in `SessionRegistry::claim` / `release_session`, not in
`on_server_disconnected` (`server.rs:676-725` clears everything **except** this).

**Failure A (normal gameplay, no attacker):** Client A drives rover R up to `seq = 5000`, releases.
Client B possesses R; its `next_seq` restarts at 1. The host keeps stamping `last_input_seq = 5000` into
every snapshot. On B, `reconcile_owned_prediction`
(`crates/lunco-sandbox-edit/src/commands.rs:1223-1249`) sees `ack = 5000`, sets
`vlog.last_reconciled = 5000`, finds no `PredictedState` with that seq → `continue`. Every subsequent
ack is `≤ 5000` → early-return at `:1230`. **B's prediction is never reconciled again, ever** —
including the `Snap` path. **Unbounded drift on the possessed rover.**

**Failure B (hostile):** one `SetPorts{seq: u32::MAX}` permanently poisons that gid for every future
owner.

**Failure C:** the map grows unbounded on a long-lived host (`gather_snapshot` prunes `last_sent`,
`repl.entries`, `spawn_info` at `sync.rs:1637-1644` — but not this).

**Fix:** key by `(gid, owner_session)` **or** zero the slot in `SessionRegistry::claim` /
`release_session`; validate `cmd.seq` against the sender's expected next seq server-side; prune
alongside `repl.entries`.

---

### N2 · HIGH · The host applies client input on the **render** clock — "post-turn oscillation" is not structurally fixed
`crates/lunco-networking/src/sync.rs:2921` registers `drain_sync_inbox` in **`Update`**;
`gather_snapshot` runs in **`FixedPostUpdate`** (`sync.rs:2992`). `SetPorts` writes **latching** ports.

A client emits exactly one `SetPorts` per `FixedUpdate` tick
(`crates/lunco-controller/src/lib.rs:199-204`). A host whose `Update` is slower than its `FixedUpdate`
(render throttle, unfocused window, load) drains **K** of them in one frame. All K write the same
ports, so **only the last survives into physics** — the host integrates **one** tick of the **last**
input. But the ack takes `max(seq)` (see `N1`), **claiming all K were applied**. The client, meanwhile,
predicted K ticks of K distinct inputs.

Divergence scales with K **and with input variability** — i.e. it appears exactly on turns and stops,
which is the reported symptom. The current mitigation is the widened dead-zone (`eps_pos: 0.40`,
`eps_rot: 0.10` — `crates/lunco-core/src/reconcile.rs:49-50`, whose comment reads *"the old thresholds
fired a tiny correction on nearly EVERY ack"*). **That is a band-aid over the cause.**

**Fix:** buffer inbound `SetPorts` per gid and consume **one frame per `FixedUpdate` tick**, acking the
seq actually integrated. Or move the command drain into `FixedPreUpdate` (keeping the ferry itself in
`Update`, which the reliable-flush comment at `server.rs:406-413` requires).

---

### N3 · HIGH · No desync detection anywhere
```
$ grep -rn 'checksum\|state_hash\|desync\|crc' crates/lunco-networking/    # → nothing in the sync path
```
No per-tick state hash, no divergence counter on the wire, no client-side "I diverged" signal. The only
backstop is `ReconcileParams::snap_pos = 6.0 m` (`reconcile.rs:51`) — a **silent** per-body snap that
only fires for `OwnedLocally` bodies on a **new ack**, which `N1` can permanently disable. Free
`PredictedDynamic` props and every kinematic proxy have **no divergence check at all**.

`docs/architecture/31` §7 concedes predicted-Dynamic desync is a known open issue — **but there is no
way to observe it in the field.**

**Fix:** host stamps a cheap rolling digest (FNV over `(gid, cell, pos_q, rot_packed)` for the peer's
interest set) on every Nth `SnapshotMsg`; client recomputes over the same set and on mismatch logs +
force-rebaselines. Export a per-body max-divergence gauge in `diagnostics.rs`.

---

### N4 · MEDIUM · No wire-version handshake on a positional, non-self-describing codec
`crates/lunco-networking/src/sync.rs:84-86` and `:352-354` **both say it outright**: *"this field is a
WIRE-BREAKING addition; peers must run the same build (no protocol-version handshake exists)"*.

`bincode` (`shared.rs:35-59`) is positional. A stale cached wasm bundle against a fresh host
mis-decodes `SnapshotEntry` field-for-field — garbage `cell` × `cell_edge` (`compose_cell_pos`,
`sync.rs:135`) **teleports every body by kilometres**. There *are* tests locking enum discriminants
(`sync.rs:3110-3179`) — good — but nothing detects a **field-layout** skew at runtime.

**Fix:** `HandshakeMsg` gains `wire_version: u32`; host refuses / client hard-errors on mismatch. The
handshake is already the first reliable message.

---

### N5 · MEDIUM · Unbounded connect-time journal replay on a reliable, unbounded-queue channel
`crates/lunco-networking/src/server.rs:657-667` — on connect the host sends **one envelope per journal
entry**, all on `BulkData` (reliable). `server_send`'s own comment (`server.rs:546`): *"lightyear's
reliable sender queues without bound"* — **the exact reason asset bytes were moved to HTTP.**

A twin with a few thousand edits (their own test `host_does_not_replay_saved_history` cites a real
**982-entry** journal) enqueues 982 reliable messages at connect — precisely when the client is fetching
assets and needs its ownership/manifest frames.

**Related:** AOI **fails open** to *every* body for a peer with no view center (`sync.rs:1698` —
`next.insert(session, all.clone())`). That is the state for the first ~200 ms of every connect, and
**permanently** for a free observer whose `ViewCenter` reports drop (they ride the *lossy*
`ControlStream`, `sync.rs:2158-2164`).

**Fix:** batch journal replay into chunked messages with a per-frame send budget; bound the fail-open to
a nearest-N cap rather than "all".

---

### N6 · LOW · Dead credential advertises a scheme that doesn't exist
`HandshakeMsg.token` is stored in `SessionCredential` (`sync.rs:229-235`, set at `:1159`) and **never
read again** by any client code path. Harmless (authority *is* correctly connection-bound), but it makes
the security posture look stronger than it is in review. Delete it or implement it.

---

# 5. PERFORMANCE

### R1 · CONFIRMED · wasm tile bakes run on the **main thread**, uncapped, and never hit cache
**`crates/lunco-terrain-surface/src/stream_viz.rs:1021-1077`**

Three compounding facts:
1. On `wasm32`, `AsyncComputeTaskPool` has **no threads** — the future runs to completion on the main
   thread the instant it's polled. **The codebase knows this**: `collider_ring.rs:224-233` says so and
   caps itself to `bake_budget = 2`. **`stream_viz` has no such cap** — `TerrainLodConfig::bakes_per_frame
   = 4` on **both** platforms (`stream_viz.rs:366`).
2. `tile_cache::bake_tile_mesh_cached` → `lunco_precompute::bake_or_load` → `load_blob`/`store_blob`,
   which are **hard no-ops on wasm** (`crates/lunco-precompute/src/lib.rs:143-155`). ⇒ on web the cache
   **always misses, every tile, every session.**
3. Cost per tile (`tile_mesh.rs:98-135`): 2401 vertex `height_at` + 4 eps-samples each for normals + 625
   parent-lattice samples ⇒ **~12k composed-oracle samples per tile**, each walking the full modifier
   chain (craters, edits, overzoom).

⇒ **4 × ~12k oracle samples on the browser main thread, per frame, while streaming.** Estimated
**5–50 ms/frame of hitching** during any camera move (`PLAUSIBLE` on the ms figure; the main-thread
execution and sample count are `CONFIRMED`). This is almost certainly the residual "web feels chunky
while moving" symptom.

**Fix:** (a) `#[cfg(target_arch = "wasm32")] bakes_per_frame = 1` mirroring `collider_ring`;
(b) `futures_lite::future::yield_now().await` between the row loops in `bake_tile_mesh` so a bake spans
frames instead of blocking one; (c) wire the wasm tile cache to `lunco_storage::opfs_blob` — the async
seam already exists.

---

### R2 · CONFIRMED · `LodMeshCache` leaks dead terrains **and then thrashes the live one**
*(found independently by two reviewers)*
**`crates/lunco-terrain-surface/src/stream_viz.rs:1119-1122`**
```rust
if mesh_cache.0.len() > CACHE_CAP {
    let resident: HashSet<QuadCoord> = tiles.tiles.keys().copied().collect();
    mesh_cache.0.retain(|(e, c), _| *e != terrain || resident.contains(c));
}
```
Re-keying the cache to `(Entity, QuadCoord)` changed the cap-trim from *"keep only resident coords"* to
*"entries belonging to any **other** entity are explicitly preserved"*. And
`despawn_orphaned_lod_tiles` (`:1141-1163`) evicts `LodMaterials` for dead terrains but **never touches
`LodMeshCache`**.

After a twin reload / scene swap (a **new `Entity`**):
- Up to `CACHE_CAP` (1024) strong `Handle<Mesh>` per dead terrain stay resident **forever**. Each tile
  mesh ≈ 2401 verts × 44 B + 55 KB indices ≈ **160 KB** ⇒ **~164 MB held per dead terrain**, unbounded
  across reloads.
- Once dead entries alone exceed `CACHE_CAP`, `len() > CACHE_CAP` is true **every frame**, so the **live**
  terrain's non-resident meshes are trimmed every frame ⇒ **the tile cache is permanently defeated** and
  every trailing-edge tile re-bakes on demand.

On `main`, the global `retain(|c, _| resident.contains(c))` swept those entries.

**Fix:** in `despawn_orphaned_lod_tiles`, take `ResMut<LodMeshCache>` and
`mesh_cache.0.retain(|(t, _), _| !orphaned.contains(t) || streaming.get(*t).is_ok())`. Make the cap
check global-aware (LRU across all terrains, or scale `CACHE_CAP` by terrain count).

---

### R3 · CONFIRMED · Tile meshes are stored twice: CPU RAM **and** VRAM
**`crates/lunco-obstacle-field/src/plugin.rs:231`** (`grid_mesh`, used by every LOD tile via
`stream_viz.rs:1073`)
```rust
Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
```
`RenderAssetUsages::default()` = `MAIN_WORLD | RENDER_WORLD` — the vertex buffers stay in
`Assets<Mesh>` **after** GPU upload. **Nothing reads tile mesh CPU data back** (physics rides the
separate collider ring; picking rides the oracle).

512 resident tiles ⇒ **~82 MB** of pointless CPU-side vertex data; the 1024-entry cache pushes it to
**~164 MB**, doubled against the same in VRAM. On wasm (2–4 GB linear-memory ceiling — cf. the 4 GB OOM
history) that is the difference between "works" and "tab dies".

**Fix:** `RenderAssetUsages::RENDER_WORLD` for the LOD tile path. Keep `default()` for the
obstacle-field slab if anything reads it back. **One line; halves terrain CPU memory.**

---

### R4 · CONFIRMED · MSAA 4× is on everywhere by default; Bloom is configured on a non-HDR camera
- **No `Msaa` component or resource exists anywhere in the workspace** (grep: zero hits) ⇒ Bevy's
  default `Msaa::Sample4`. On WebGL2 that is a 4× multisampled colour+depth target for a full-screen
  terrain — **the single biggest free win available on web** (≈1.5–3× fragment/bandwidth cost;
  `PLAUSIBLE` on the multiplier, hardware-dependent — the default is `CONFIRMED`).
- **`crates/lunco-celestial/src/big_space_setup.rs:484-505`** spawns `Camera::default()` (⇒ `hdr:
  false`) **plus** `Bloom { .. }`. `hdr` is set true **nowhere in the repo**. Four crates configure
  `Bloom` (`lunco-sandbox`, `lunco-sandbox-edit/ui/inspector`, `lunco-environment`, `lunco-celestial`) —
  all against a camera with no HDR target. Bloom on a non-HDR view is at best a no-op with a
  downsample/upsample chain attached.

**Fix:** `Msaa::Off` on wasm (the shader's `aa_fade` footprint fades already do the AA that actually
matters here), `Msaa::Sample2` native. Either set `hdr: true` on the camera or delete the four `Bloom`
configs. Verify with a frame capture.

---

### R5 · CONFIRMED · Reveal tweening breaks batching and re-uploads a material every frame
**`crates/lunco-terrain-surface/src/stream_viz.rs:603-647`**

`begin_reveal` **clones the shared material into a per-tile transient** (`materials.add(anim)`), and
`animate_tile_reveal` calls `materials.get_mut(&rev.anim)` every frame for 0.35 s. Each `get_mut` marks
the asset modified ⇒ **uniform buffer + bind group re-prepared that frame**, and the tile can no longer
batch with its depth-mates.

Steady state during motion: `bakes_per_frame(4) × REVEAL_SECS(0.35) × 60 fps` ≈ **84 unique materials
alive** ⇒ **84 extra draw calls + 84 bind-group/uniform re-uploads per frame**.

**Secondary:** those `AssetEvent::<ShaderMaterial>::Modified` events fire every frame, which defeats the
`mat_changed` early-out in `crates/lunco-materials/src/shader_material.rs:837-842` and forces a **full
`mats.iter()` sweep every frame** while any tile is revealing.

**Fix (surgical):** bucket `reveal` into ~8 quantised **shared** materials instead of one clone per
tile — 84 unique → 8 shared, batching restored. **(Proper):** move reveal out of the material uniform
into a per-instance value the shader reads via `instance_index`, or fold it into the distance-morph path
(which is already per-band-bucket shared).

---

### R6 · CONFIRMED · The obstacle-field 43×-FPS landmine is **still armed**
**`crates/lunco-obstacle-field/src/plugin.rs:203-207, 216-220`**
```rust
impl Default for ObstacleFieldMode { fn default() -> Self { Self::Standalone } }  // ← the expensive path
fn trigger_initial(mode: Res<ObstacleFieldMode>, mut ev: MessageWriter<RegenerateField>) {
    if mode.is_standalone() { ev.write(RegenerateField); }                        // ← fires at Startup
}
```
**The 43× fix was made in the consumer** (the sandbox sets `DemDelegated`), **not in the plugin.** Any
app/scene/test that adds `ObstacleFieldPlugin` and forgets the resource gets, at `Startup`: a full slab
heightfield + `sample_layer` rock scatter spawning **one ECS entity + one static `Collider::sphere` per
rock** (`plugin.rs:417-427`).

And unlike `terrain_layers::rocks` — which has an explicit `#[cfg(target_arch = "wasm32")] return;` web
bail (`rocks.rs:73-82`) — **this crate has no web skip and no `VisibilityRange` on wasm**
(`plugin.rs:245-252`). On the browser, every rock renders and sits in the avian broadphase every frame,
on the single thread. That is precisely the 43× regression shape.

The code even carries a `TODO: remove Standalone` (`plugin.rs:190-196`) saying no production path reaches
it.

**A default that is the pathological path is not a fix, it's a fuse.**
**Fix:** make `DemDelegated` the `Default`, or execute the TODO and delete `Standalone` outright.

---

### R7 · CONFIRMED · No LOD hysteresis; the budget warm-start hunts
**`crates/lunco-terrain-surface/src/stream_viz.rs:836-881`** + `crates/lunco-terrain-core/src/quadtree.rs:390-416`

Refinement is a bare `dist < refine_range` test — **no hysteresis band.** A camera hovering on a
boundary flips a node in/out **every frame**. The mesh cache absorbs the *bake* cost, but each flip
still costs a despawn + spawn + `begin_reveal` (⇒ a fresh transient material — see `R5`) + a 0.35 s
reveal animation **on a tile that never actually changed LOD**.

Separately, the budget warm start (`last_fit_px → px / 1.6`) re-walks the quadtree 1–2× per selection at
steady state and, near the budget boundary, **alternates between two `pixel_error` rungs** — which
changes `morph_end` → changes `band_bucket` → **swaps every tile's material on alternating frames.**

**Fix:** hysteresis on `refine_range` (refine at `r`, coarsen at `1.15·r`) and on the budget fit (only
coarsen above `budget`, only refine below `0.85·budget`).

**Also in this system, per frame per terrain:** `swaps: Vec` (`:732`), `done: Vec` (`:978`),
`keyed: Vec` (`:949`), `missing: Vec` (`:1086`), `wanted: HashSet` (`:898`) — **five heap allocations per
frame.** The idle-signature gate (`:780-817`) is an excellent mitigation and skips all of it when still,
but any motion pays. Hoist into `Local<Vec<_>>` scratch buffers.

---

### R8 · CONFIRMED · Near-field fragment cost: ~160 `hash13` per pixel across most of the screen
**`assets/shaders/terrain_geomorph.wgsl:331-379`**

`bump_layer` calls `layer_height` **3×** (h0, ht, hb), each an `fbm`, each octave an 8-tap `vnoise`.
Layers live at eye height:

| layer | `aa_fade` cuts out at | octaves × 3 | vnoise calls |
|---|---|---|---|
| meso (`mid_scale` 0.45) | pw > 0.74 m ⇒ **d ≳ 960 m** | 3×3 | 9 |
| sub-meso (×3) | d ≳ 320 m | 2×3 | 6 |
| tooth (`macro_clump_scale` 8) | d ≳ 54 m | 3×3 | 9 |
| fine (180) | d ≳ 2.4 m | 2×3 | 6 |
| dust (0.004) | never | | 3 |
| grain (0.35) | d ≳ 1.2 km | | 2 |

⇒ **~20 `vnoise` = ~160 `hash13` per fragment** across essentially the whole visible ground out to
~1 km. The doc comment says the far field is *"two texture samples"* — true beyond ~1 km, **but the 1 km
disc *is* the screen when you are standing on the surface.** Estimated **3–8 ms/frame at 1080p on an
iGPU** (`PLAUSIBLE` on the ms; the tap count is `CONFIRMED`). Likely the dominant remaining web fragment
cost after MSAA.

**Fix:** the baked normal-map machinery **already exists** (`tile_map_weights`, `stream_viz.rs:484-490`)
— it is just tuned to fade the map *out* exactly where the expensive FBM fades *in*. Cross them over
earlier, and/or tighten `aa_fade`'s handoff (the `/9.0` ramp at `:175` is generous).

---

### R9 · CONFIRMED · `RockInstanceLayer::scatter` allocates a new `Mesh` **and** `StandardMaterial` per rock
**`crates/lunco-terrain-surface/src/terrain_layers/rocks.rs:236-246`** — each `PlaceRock` command
permanently adds a draw call + a bind group.

**The procedural rock path is exemplary by contrast** (`rocks.rs:118-137`): 6 shared bucket meshes + **1
shared material** ⇒ ~6 draws for up to `MAX_ROCKS = 6000`, `NotShadowCaster`, `VisibilityRange`-culled
on native, skipped on web with a clear rationale. **Fix:** make `scatter` share one boulder material and
bucket its meshes the same way.

---

### R10 · CONFIRMED · A worker crash poisons the bake pool: every subsequent bake hangs forever
**`crates/lunco-terrain-bake/src/worker_client.rs:77-102`** (+ `:66-108`)

The new `on_error` handler drains `INFLIGHT` and synthesises error replies — good — but it **never
respawns the worker**. `WorkerPool` explicitly delegates that to the caller
(`crates/lunco-worker-transport/src/lib.rs:38-42`: *"The caller should schedule a DEFERRED
`WorkerPool::respawn`"*), and `make_worker` leaves `slots[idx] = Some(dead_worker)` after `onerror`.

Failure: DEM worker OOMs/panics on bake #1 → `on_error` fails that job (this diff's fix) → user hits
"Regenerate" → `dispatch` → `ensure_pool()` → `ensure(1)` sees slot 0 occupied → `post_transfer(0, …)`
into the **wedged** worker → **no reply, no second `onerror`** → the id sits in `INFLIGHT` and the
terrain is pending **forever** (and a fresh 40 MB TIFF is transferred into the dead worker on every
attempt). **The fix is half-done.**

**Fix:** set a `NEEDS_RESPAWN` thread-local in `on_error` and, on the next `dispatch`/`drain_replies`,
call `respawn(0)` (deferred, as the transport contract requires) before posting.

**Related, same file:** `INFLIGHT` ids **leak** when a job's entity is despawned before the `Full` reply
arrives (`:45-51`, `:156-159`) — only `Full` or an error retires an id. The set grows across a session,
`retire_inflight`'s `retain` is O(n) per reply, and the "failing N in-flight bake(s)" count lies.
**Also `:139`:** `header.res * header.res` can overflow `usize` on wasm32 (32-bit) — the guard added to
defend against a truncated/foreign buffer can itself be defeated by `res ≥ 65536`. Use `checked_mul`.

---

### R11 · CONFIRMED · `CarveField::new` panics on a non-finite primitive
**`crates/lunco-terrain-core/src/carve.rs:141`** (and `:150-165`)

`cell_of` does `(x / cell_size).floor() as i64`, which **saturates** for ±inf. A single
`CarvePrimitive::Sphere { radius: f64::INFINITY }` (a scripted or USD-authored value divided by zero)
gives `x0 = -inf, x1 = +inf` ⇒ `min_cx = i64::MIN`, `max_cx = i64::MAX`:

- **debug:** `max_cx - min_cx + 1` at `:141` → *"attempt to subtract with overflow"* panic.
- **release:** wraps to `-1`, `+1 = 0` ⇒ `nx = nz = 0`, `counts = vec![]` — but the CSR fill loop at
  `:155-165` does **not** hit the `x0 > x1` guard (`i64::MIN <= i64::MAX`) and immediately indexes
  `counts[slot(cx, cz)]` on an **empty Vec** → OOB panic.

**Fix:** reject/skip primitives whose `xz_bounds()` are non-finite (or clamp them); compute the span in
`i128` / `saturating_sub`. **`crates/lunco-terrain-core/src/crater.rs:292-308` carries the same
pattern** (pre-existing) — guard it too.

**Also (memory):** at the `MAX_BUCKET_CELLS = 1<<21` ceiling, `counts` + `starts` + `cursor` are ~24 MB
of transient allocation **per `CarveField::new`**, and the field is rebuilt on **every carve edit**.
Counting into `bucket_starts` directly (dropping the separate `counts` Vec) removes a third of that for
free.

---

### R12 · CONFIRMED · The `target_pixel_error` guard does not prevent the blow-up it claims to
**`crates/lunco-terrain-core/src/quadtree.rs:125-128`**

`target_pixel_error.max(1e-3)` only avoids `inf`/`NaN` arithmetic. At 1e-3 px the `range_factor` is
~1000× the sane value, so **every node still refines to `max_depth`** — the exact "triangle/tile
blow-up" the comment says it stops. And the other divisor is unguarded: `fov_y_rad = 0` ⇒
`sse_denominator = 0` ⇒ `range_factor = inf` ⇒ same blow-up, plus `inf * 0 = NaN` downstream.

**Fix:** clamp to a *usable* floor — `.clamp(0.25, 64.0)`, matching `stream_viz.rs:832`'s own
`clamp(0.5, 32.0)` — and floor `sse_denominator` at ~`1e-4`.

---

### R13 · Other perf items worth queuing
- **`crates/lunco-modelica/src/worker_transport.rs:1240`** — `TODO(CQ-213): parsed.to_vec()
  deep-clones the full ~165 MB parsed`. Shipped with the TODO.
- **`crates/lunco-terrain-bake/src/lib.rs:124-125`** — `let base_grid = tile.clone();` copies the whole
  working grid (`res² × f64`; with `detail_upsample` ≈ 1537² × 8 B ≈ **19 MB**) on **every** bake —
  including the `Coarse` stage (thrown away when `Full` lands) and jobs where `job.stamps.is_empty()`
  (so `base_grid == grid` bit-for-bit). Skip the clone when stamps are empty (share one `Arc`), and skip
  it entirely for `BakeStage::Coarse`.
- **`crates/lunco-terrain-core/src/field.rs:64-84`** — `field_map(&dyn SurfaceField, &dyn HeightSource,
  …)` pays a vtable call per texel **and** a vtable `height_at` per finite-difference sample (4 per texel
  for `SlopeField`), while `derive::slope_map<S: HeightSource>` is generic and inlines the whole chain —
  and the two produce **identical output** (the crate's own test at `field.rs:167-169` asserts it). For a
  512² field that's ~1M dynamic calls `slope_map` doesn't make. Also serial despite being embarrassingly
  parallel per row. Bounded impact today (on-demand only), but it will be the hot loop the moment it
  backs a per-tile overlay texture. **Fix:** make it generic over `S: HeightSource`; `par_chunks_mut` the
  rows behind the existing rayon feature.
- **`crates/lunco-terrain-core/src/carve.rs:208,277-279`** — the struct doc calls `sdf`/`is_open` *"the
  baker + collider inner loop"*, then `cell_of` does **two f64 divisions** by `cell_size` per sample.
  Store `inv_cell_size` and multiply. Free win in the loop this whole CSR rewrite exists to speed up.
- **`crates/lunco-avatar/src/lib.rs:2530-2598`** — `update_avatar_clip_planes_system` writes
  `*projection` **unconditionally** every `PostUpdate` ⇒ `Changed<Projection>` fires every frame ⇒
  frustum recompute + view-uniform re-upload even for a static camera. Guard with an epsilon compare.
- **`crates/lunco-terrain-surface/src/query.rs:438-460`** — `TerrainField` does up to **262k oracle
  evaluations on the main thread** (`res` caps at 256 ⇒ 65,536 texels × ~4 `height_at`), inside
  `execute(&mut World)`, plus a ~600 KB JSON array. A single `query("TerrainField", #{res: 256})`
  visibly stalls the render loop on a modifier-heavy oracle.
- **`crates/lunco-terrain-core/src/error.rs:48-53,58-90`** (`PLAUSIBLE`) — `measure_node_error` holds a
  `RefCell::borrow_mut` across every `src.height_at()` call. Any `HeightSource` that transitively
  re-enters panics with `BorrowMutError` instead of merely being slow. Latent (the only caller doesn't
  recurse) but the trait is public and the invariant is undocumented. `take()` the Vec out for the
  duration so re-entrancy degrades to an extra alloc, not a panic.

---

# 6. THE BRANCH DIFF ITSELF (regressions introduced by `optimization`)

### D1 · `SetTerrainOverlay` turns the overlay OFF when you try to re-tune it
**`crates/lunco-terrain-surface/src/overlay.rs:219-234`**

The doc says *"a field left at its default 0 is treated as keep the current value"* — true for the three
floats, but `params.enabled = ev.enabled` is **unconditional**, and `#[Command(default)]` ⇒
`enabled: false`.

⇒ `{"command":"SetTerrainOverlay","params":{"cliff_deg":25}}` from MCP or rhai **silently turns the
overlay off** while appearing to be a re-tune. (Same class: the zero-sentinel means **you can never set
`opacity` to 0** — the command keeps the old value.)

**Fix:** `Option<T>` fields. `#[Command(default)]` + `#[serde(default)]` already gives "omitted"; the
struct just needs to be able to *represent* it. The reflect deserializer handles `Option` fine — see
`resolve_command_ids`'s `Option<Entity>` path.

---

### D2 · The traversability overlay shades from the **LOD mesh normal**, so it lies at distance
**`crates/lunco-terrain-surface/src/overlay.rs:1-18`** (doc) + **`assets/shaders/terrain_geomorph.wgsl:451`**

The module doc asserts *"the pixel colour matches a legend swatch or a headless export exactly."* **It
cannot.** The shader does `acos(n_geo.y)` on the **interpolated vertex normal**, and tile normals are
sampled from `oracle.detail_limited(step)` with a fixed `eps = 0.5 m` per depth
(`tile_mesh.rs:73-91`) — whereas `TerrainField` / `SlopeField` uses `eps = cell size` on the
**un-band-limited** oracle (`crates/lunco-terrain-core/src/field.rs:72`).

Meanwhile the data plane **already carries the DEM-resolution hazard in `surface_tex.a`**
(`pack_surface_rgba8`: A = hazard) — and the overlay **ignores it**.

⇒ **A 35° cliff on a far/coarse tile shades green/safe, and re-colours as the camera approaches and the
LOD refines.** For a traversability overlay that is the exact opposite of the required failure
direction. This is also the architectural answer to *"does the view reach around the field
abstraction?"* — **yes: the VIEW re-derives the field from render geometry instead of consuming the
`SurfaceField` data plane.**

**Fix:** sample slope from the baked normal/hazard map (`map_n.xyz` / `map_s.a`) when
`weight_normal > 0`. At absolute minimum, delete the "matches exactly" claim and document the LOD
dependence.

---

### D3 · The BT.CPP v4 XML codec loses data on realistic inputs
**`crates/lunco-autopilot/src/btcpp_xml.rs`** — highest-risk new file in the diff. It must round-trip
JSON → XML → JSON without loss. It does not.

| # | site | defect | failing input |
|---|---|---|---|
| 1 | `:155-163`, `:342-358` | `attr_from_value` joins nested arrays with `\|`; a **1-element** vec-of-vecs produces **no separator**, so the nesting level is lost | `{"kind":"patrol","waypoints":[[10.0,0.0,-5.0]],…}` → `waypoints="10;0;-5"` → decodes as a flat `[f32;3]` → `BehaviorSpec::from_json` (`lib.rs:914`) errors *"invalid type: floating point 1.0, expected an array of length 3"*. Empty `"waypoints":[]` becomes the string `""`. |
| 2 | `:313-318` | the `other =>` arm in `frame_to_value` **ignores the `children` parameter entirely** — unknown elements **silently drop their whole subtree** | any real Groot2/BT.CPP file with `<Delay>`, `<IfThenElse>`, `<WhileDoElse>`, `<Switch2>`, `<KeepRunningUntilFailure>`, `<Precondition>`. Same defect on the write side (`write_leaf`, `:140-142`). **This falsifies the module doc's claim (`:11-15`) that "any kind added later round-trips without touching this file".** |
| 3 | `:211-215` | `Event::Empty` skips the `is_transparent` check (only `Event::End`, `:219`, consults it) | `<root BTCPP_format="4"><BehaviorTree ID="MainTree"/></root>` → `{"kind":"behavior_tree","ID":"MainTree"}`; `<root BTCPP_format="4"/>` → `{"kind":"root","BTCPP_format":4}`. Both should error "no behaviour-tree node found". |
| 4 | `:219-224` | transparent `<root>` bubbles only `children.next()` — **multi-tree files lose every tree but the first**, and `main_tree_to_execute` is **ignored** | `<root main_tree_to_execute="Main"><BehaviorTree ID="Helper">…</BehaviorTree><BehaviorTree ID="Main">…</BehaviorTree></root>` → returns **Helper**, silently. Real BT.CPP files with `<SubTree>` always look like this. |
| 5 | `:169-174` | `int_field` uses `Value::as_i64`, which returns `None` for `3.0` — silently yields **0** | `{"kind":"repeat","times":3.0,…}` → `<Repeat num_cycles="0">` → **the repeat is destroyed.** Hand-authored/rhai JSON hits this trivially. |
| 6 | `:82-87`, `:298-301` | `sub_tree` is a **phantom kind** — `BehaviorSpec` has no `SubTree` variant | the write arm is unreachable dead code; any imported `<SubTree ID="X"/>` yields `{"kind":"sub_tree"}`, which `SetAutopilotBehavior` rejects with *"unknown variant `sub_tree`"*. Same class: `<AlwaysSuccess/>` imports as `always_success` while the spec's kind is `succeed`. |
| 7 | `:342-366` | `value_from_attr` **guesses** types — strings that look like numbers/bools/vectors don't survive | `"text":"42"` → `42`; `"name":"true"` → `true`; **`"note":"NaN"` → `null`** (`Number::from_f64(NaN)` → `None`), same for `inf`. Latent (no `String` field in any spec variant today) but the contract claims genericity. |
| 8 | `:45`, `:196` (`PLAUSIBLE`) | no depth cap. `xml_to_value` is iterative, but the resulting `Value` is `serde_json::to_string`'d (`lib.rs:1283`), which **is** recursive | 50k nested `<Inverter>` in an `ImportBehaviorXml` payload → **stack overflow, process abort**. (Export is protected by serde_json's 128-depth limit; import is not.) |
| 9 | `:73`, `:286-290` | integer-valued `seconds` breaks the tests' own `assert_eq!(v, back)` contract | `{"kind":"timeout","seconds":5}` → `msec="5000"` → `"seconds":5.0` (f64 ≠ i64). `seconds: 0.0004` → `msec="0"`. `{"kind":"invert"}` → `{"kind":"invert","child":null}`. |
| 10 | `:144` (`PLAUSIBLE`) | quick-xml's `escape()` covers `< > & ' "` but **not `\n` / `\t`** | XML attribute-value normalization (spec §3.3.3) turns those into spaces in Groot2/tinyxml2/lxml ⇒ the JSON→XML→external-editor→JSON path loses them. Escape numerically (`&#10;`, `&#9;`). |
| 11 | `:57-58` | `<Parallel require="all\|one">` **is not BT.CPP v4** — that library uses `success_count`/`failure_count`. Groot2/Nav2 will not honour `require`. | map `all` → `success_count="-1"`, `one` → `success_count="1"`. (`Cooldown sec=` is knowingly custom and documented — fine.) |
| 12 | `:302-311` | `<Action ID="wait" kind="pwn"/>` — the attribute loop skips only `"ID"`, so a wire attribute can **overwrite the `kind`** set at `:304` | skip/reject `kind`, `child`, `children` as attribute names. |
| 13 | `:408-469` | **5 tests, all happy-path, all float-typed scalars.** None cover `patrol` (the broken kind), `sub_tree`, `cooldown`, empty trees, `<root/>`, malformed XML, unknown elements with children, string attributes, integer-typed `seconds`/`times`, or XML-escape chars. | **Add the `patrol` 1-waypoint and 0-waypoint tests first — they fail today.** |

**Structural fix (do this, not 13 patches):** make the mapping table **exhaustive over the
`BehaviorSpec` enum** — match on the enum, not on a free-form `&str` — so the compiler catches new
variants. Right now the table is a **second source of truth**, and finding #2 is the direct consequence.
Encode arrays unambiguously (emit JSON text for array/object values, decode by shape not by "contains
`;`").

**Architecture note:** the codec's placement in `lunco-autopilot` is defensible (it encodes
`BehaviorSpec`'s `kind` vocabulary, which lives there). But if it is meant to be reusable next to the
`lunco-behavior` kernel, only the control/decorator half belongs there; the leaf half
(`CONDITION_LEAVES`, `:130`) is autopilot-domain.

---

### D4 · `transfer.rs::TransferFn` has zero consumers; the live path duplicates it in WGSL
**`crates/lunco-terrain-core/src/transfer.rs:46-117`**

Repo-wide, the only things imported from `transfer` are `hazard_color` + `hazard_from_slope` (the legend
— `crates/lunco-sandbox-edit/src/ui/inspector.rs:1081`). **`TransferFn` (all four variants), `sample`,
and `sample_palette` are used only by their own unit tests.**

Meanwhile `assets/shaders/terrain_geomorph.wgsl:452-463` **re-implements the smoothstep and hard-codes
the same RGB triples** as `HAZARD_SAFE/WARN/CLIFF` (`vec3(0.15,0.75,0.20)`, `vec3(0.90,0.15,0.10)`).

So the module doc's *"one definition, many consumers … the whole function travels as uniforms"* **is not
realised**: the colours are a copy-paste, and changing `HAZARD_CLIFF` in Rust **silently desyncs the
terrain from the legend.**

Same class in `field.rs`: `FieldKind`, `SurfaceField::kind()`, `SurfaceField::content_key()` are
**never called** anywhere (`query.rs` uses only `field_by_id` + `field_map`). Dead trait surface.

**Fix:** either push the three swatches to the material as uniforms (the *angles* already are —
`overlay_safe_rad` / `overlay_cliff_rad` — just extend it to colours), **or delete `TransferFn` /
`Palette` / `Threshold` / `Ramp` until a consumer exists.** Landing an unused enum ahead of its consumer
is exactly the parallel-path duplication this review was looking for.

---

### D5 · Horizon re-bake debounce silently drops edits made while a bake is in flight
**`crates/lunco-sandbox/src/terrain_horizon.rs:154-159`**

`start_streamed_horizon_bakes` removes `StreamedHorizonStale` and inserts `StreamedHorizonBake`. While
that task runs, an edited terrain has `has_map == false` **and** `is_stale == false` ⇒
`mark_streamed_horizon_stale` `continue`s. When the bake lands, `HorizonMap` is installed from the
**pre-edit** oracle snapshot and **nothing re-arms.**

⇒ Brush a crater ~0.8 s after a horizon bake starts (a few-second bake on a big DEM) and stop editing —
**the far-field sun-visibility cache stays wrong for the rest of the session.** No further
`DemHeightField` change ever fires.

**Fix:** also match `Has<StreamedHorizonBake>` and re-arm `StreamedHorizonStale` for it; let
`start_streamed_horizon_bakes` treat "map present + stale armed" as a re-bake trigger.

---

### D6 · The viewport pick gate ships with a branch its own TODO calls broken

**6a — `dock_rect` is not the dock's rect; the chrome blanket swallows clicks.**
`crates/lunco-workbench/src/lib.rs:3328` — `let dock_rect = viewport_ui.min_rect();` where `viewport_ui`
is the **root background Ui spanning the whole window** (`lib.rs:2469`), into which the menu bar
(`:2477`) and status bar (`:3192`) are also drawn. After `DockArea::show_inside`, `min_rect()` ≈ **the
entire window**.

⇒ `in_dock` is true everywhere, and
`over_chrome = … || (in_dock && !on_leaf && !in_gap)` becomes a **window-wide blanket**: any bare
full-window 3D not covered by a dock leaf is misclassified as chrome and **its clicks are swallowed**.
`crates/lunco-workbench/src/viewport.rs:641-651` carries a `TODO(pick-gate)` admitting *"does NOT work
yet"*. It also makes `over_card` redundant (leaf bodies are disjoint, so `in_dock` subsumes it) — the
whole `chrome_cards` machinery contributes only `in_gap`.
**Fix:** derive the dock extent from the union of the recorded leaf `body` rects (already collected),
or drop `dock_rect`/`in_dock` and rely on `over_card` + `is_pointer_over_egui`.

**6b — drag ownership flips mid-drag.**
`viewport.rs:767` (`track_egui_focus`) — the new `wants_pointer = !pointer_over_scene()` is **pure
geometry**. The old `egui_wants_pointer_input()` was `is_using_pointer() || (over_area && !any_down)`;
**the `is_using_pointer()` term (egui is actively dragging a widget) was dropped and nothing replaced
it.** Both consumers (`crates/lunco-avatar/src/lib.rs:1758` `capture_avatar_intent`, `:1800`
`collect_camera_zoom`) re-evaluate every frame.

- Press an egui slider / drag a dock separator, hold, move over the transparent viewport leaf ⇒
  `wants_pointer = false` ⇒ **the scene camera starts orbiting while you are still dragging the
  widget**, and bevy_picking hover/click hits fire into the scene under the cursor.
- Mirror case: press in the 3D scene to orbit, drag into the inspector ⇒ **the camera freezes
  mid-orbit** until the cursor comes back.

Both are **regressions from `main`**, where `&& !any_down()` kept the scene owning the pointer for the
whole drag. **Fix:** latch pointer ownership on press — compute `pointer_over_scene` only when no button
is down; while `any_down()`, hold the value captured at press (and/or OR `is_using_pointer()` into
`over_chrome`).

**6c — opaque panels are recorded as transparent gaps.**
`crates/lunco-workbench/src/lib.rs:2186-2194, 2216-2222` — `record_chrome_panel(body, ui.min_rect())` is
called for **every** non-scene panel, **without consulting `Panel::transparent_background()`** (default
`false` — `panel.rs:238` — ⇒ egui-dock paints an **opaque** `tab_body.bg_fill` across the whole leaf).

⇒ For any panel whose content is shorter than the leaf, `body − min_rect()` is an **opaque painted
region** that the gate (`viewport.rs:262`, `pointer_in_transparent_gap`) classifies as `in_gap` ⇒
scene. In the sandbox "Design" workspace (which embeds the lunica workbench while `ViewportPanel` is a
parked dock tab, so the 3D camera stays active — `viewport.rs:452`), **clicking or scrolling the empty
lower half of an opaque Modelica panel deselects/picks in the hidden 3D scene and zooms the scene
camera.**
**Degenerate sub-case:** a panel that early-returns without allocating leaves `ui.min_rect()` a
zero-size rect at the leaf top-left ⇒ **the entire panel body reads as a transparent gap.**
**Fix:** only `record_chrome_panel` when `panel.transparent_background()`; otherwise record
`(body, body)` so no gap exists.

**6d — the USD image preview claims to be a scene viewport.**
`crates/lunco-usd/src/ui/viewport.rs:667-691` — `UsdViewportPanel` renders a camera to an **offscreen
`Image`** and shows it as an `egui::Image` with its **own** `Sense::click_and_drag()` orbit handling
(`:730-744`). It is not the full-window 3D scene. But it returns `is_scene_viewport() == true` and calls
`record_scene_panel(over_scene)`, writing into the **same single global** `PanelRects::pointer_over_scene`
that gates the main scene ⇒ **dragging the preview double-drives the main avatar camera**, and
bevy_picking mesh hits fire *behind* the preview image. (Also `scene_pointer_from_ui` is measured with
`available_rect_before_wrap()` **before** the panel paints its title row — so the panel's own header is
inside the "scene" region.)
**Fix:** `is_scene_viewport() == false` (it handles its own input), or make `pointer_over_scene` a
per-scene-target value rather than one global bool.

**6e — one field, two meanings; stale feedback on skipped egui frames.**
`viewport.rs:124` (field), `:229` (raw OR-fold), `:234` (`set_pointer_over_scene`), `:663` (read as "raw
fold"). The doc comment concedes it. `reset_pointer_over_scene()` is called **only** from
`render_workbench` (`lib.rs:2066`, in `EguiPrimaryContextPass`), while `resolve_scene_pointer` runs
unconditionally in `PostUpdate`. **Any frame the egui pass is skipped** (window unfocused/occluded/
minimized, or a perspective that doesn't call `render_workbench`), `resolve_scene_pointer` reads
`on_leaf = !over_chrome_from_last_frame` — **a self-referential feedback term** — while `chrome_cards`
and `dock_rect` go stale rather than empty.
**Fix:** two fields (`raw_on_scene_leaf` written by render, `resolved_over_scene` written by resolve);
reset the per-frame inputs from a system guaranteed to run every frame, or `run_if(egui_pass_ran)` on
resolve.

**6f — legacy remnants that should have gone with this rework.**
- `viewport.rs:157` — `PanelRects::clear()` has **no callers**, and its doc references
  `clear_panel_rects_each_frame`, **a system that does not exist anywhere in the tree.** The `rects` map
  is therefore **never cleared**: a closed/perspective-switched panel **leaks a stale rect forever**, and
  the only live consumer (`resize_viewport_image`, `crates/lunco-usd/src/ui/viewport.rs:432`) keeps
  sizing the offscreen image to a rect from a panel that is no longer shown.
- `viewport.rs:173` — `record_from_ui()` has no callers (superseded by `panel_rect_from_ui` + `defer` +
  `record`).
- `crates/lunco-workbench/src/panel.rs:307` — `InstancePanel::is_scene_viewport()` has **no
  implementor**. Dead trait method.
- `crates/lunco-modelica/src/ui/panels/canvas_diagram/node.rs:147` —
  `IconNodeVisual::parent_qualified_type` is written (`mod.rs:130`) but **never read** after the
  `candidates` scope-walk was deleted. Dead field + a per-node `String` clone every projection.

**6g — `PLAUSIBLE`:** `viewport.rs:658` uses `pointer_interact_pos()`, which egui deliberately keeps
alive after `PointerGone` — so with 6a's blanket, `wants_pointer = true` **while the pointer is off the
window entirely.** Use `hover_pos()` (or `latest_pos` + an explicit gone-check).

**Architecture verdict on this subsystem:** there is **no single source of truth** for "the pointer is
over *which* scene". The resource is `pointer_over_scene: bool`, not `over_scene: Option<SceneTarget>` —
and 6d is the direct consequence. `PanelRects` now holds four unrelated things (persistent
physical-pixel rects for camera sizing, a per-frame bool, a per-frame `Vec<(Rect,Rect)>`, a per-frame
`Option<Rect>`) with **three different reset lifetimes and two coordinate spaces** (physical px vs egui
points). Split the per-frame pick-gate inputs into their own `ScenePickGate` resource — that makes the
reset/ordering contract self-evident and kills 6e outright.

---

### D7 · Brush ghost transform: the comment claims the opposite of what the code does
**`crates/lunco-sandbox-edit/src/terrain_tools.rs:224-227`** — `*tf = transform` goes through `DerefMut`,
so `Changed<Transform>` fires **every frame while armed**, regardless of cursor motion; the inline
comment claims the opposite. (The material guard immediately below it *is* correct.)
**Fix:** `tf.set_if_neq(transform);`

---

### D8 · `sync_terrain_overlay` writes overlay params into flat/debug materials too
**`crates/lunco-terrain-surface/src/overlay.rs:247-261`** — it iterates **all** of `LodMaterials`,
including `DebugLod`/`Plain` materials whose shader (`terrain_geomorph_flat.wgsl`) has no such params.
`set_value` inserts 4 new `String` keys per material and `repack()` runs; `schema.pack` ignores them, so
it's harmless — but it **marks those assets dirty ⇒ pointless uniform re-uploads.**
`build_tile_material` correctly gates on `Lit`; the sync path doesn't.
**Fix:** filter on `mode == TerrainShaderMode::Lit`, like `bind_derived_maps_to_tiles` does.

---

### D9 · `PLAUSIBLE` · Web full-res refine never updates `DemBaseGrid`
**`crates/lunco-terrain-surface/src/terrain.rs:1330-1360`** vs **`:1197-1204`** — `assemble_dem_build` is
the only place `DemBaseGrid(grid, grid_key)` is inserted, and on web that runs for the **coarse**
preview. The `BakeStage::Full` path swaps `DemHeightField` to a new full-res oracle but **leaves
`DemBaseGrid` pointing at the coarse grid**. A subsequent brush stroke's `spawn_restamp_task`
re-composes from that coarse base ⇒ **on web, sculpting after the refine lands visibly reverts the
terrain to coarse DEM heights.**

Pre-existing on `main` (the grid was already coarse); this diff only adds the matching stale key, so the
key/grid pair stays *consistent*. Flagged because this branch touches the restamp path.
**Fix:** re-insert `DemBaseGrid(full_grid, grid_key(&full_grid))` in the `Full` swap path.

---

### D10 · Notes on the diff that are **correct** (do not "fix" these)
- **`crater.rs::octave_of`** bit-exponent rewrite is correct for all inputs (`max(1e-9)` guarantees a
  normal f64; NaN → `1e-9`; ±inf clamped). It **does** change the octave for radii within 1 ULP of a
  power of two versus `main` — intentional, but it **invalidates previously-cached content hashes and
  baked colliders.** Expect a one-time re-bake.
- **`carve.rs`'s CSR grid preserves the ascending-primitive fold order** of the old `HashMap`, so the
  (non-associative) `smin` fold is **bit-identical to `main`.** Good.
- **`terrain_geomorph_web.wgsl` is NOT a stale copy.** Diffed in full: `struct Material` and the
  `//!@default` block are **identical**, including the four new `overlay_*` fields (no layout drift —
  which would silently mis-pack uniforms). The overlay block is byte-identical apart from comments. The
  divergences are **intentional WebGL perf choices** (2D `hash12`/`vnoise2d`/`fbm2d` on `p.xz`, 1 octave
  per bump layer, vs native's 3D noise with 2–3 octaves and a golden-angle domain rotation), all
  documented. *(The hazard-palette duplication is real — see `D4` — but the two files do not disagree
  with each other.)*
- **`SurfaceOracle::new_with_base_key` + `DemBaseGrid.1`** — the cached key is only ever produced by
  `grid_key(&base)`; the stamping branch correctly re-folds; `surface_key()` still mixes `base_key`, so
  tile-bake cache keys stay sound.
- **`TerrainGenPhase` typed enum** correctly removes the per-frame `String` alloc and the fragile
  substring match; the `Baking` arm *is* reachable, so the native caption bug is genuinely fixed.
- **`lunco-terrain-globe/src/tile.rs:52-54`** — the `y.clamp(-1,1).asin()` guard is a correct NaN-UV fix.
- **`TerrainSurfaceConfig` removal** — no dangling references anywhere in `crates/` or `docs/`.
- **`stamp.rs`** change is a pure constant-extraction; behaviour is byte-identical.
- **`world_bridge.rs:359-373`** stays runtime-agnostic (delegates to `bridge_core::world_rotation`, no
  Bevy types leak into the rhai layer). `bridge_core::world_rotation` (`:659-666`) is correct, and the
  `qrot`/`world_up`/`tilt_deg` prelude (`assets/scripting/prelude/math.rhai:41-68`) implements the
  quaternion rotation correctly. Catalog entry present.
- **`lunco-tutorial`'s `PendingAdvance` clearing** (`lib.rs:241,268`) is correct — the resource is
  `init_resource`d at `:694`, and `draw_advance_prompt`'s Continue path triggers `StartTutorial` **before**
  the deferred closure clears the prompt. No ordering hazard.

---

# 7. BUILD & HYGIENE

### H1 · 🔴 **clippy has never run on the five biggest crates**
```
$ cargo clippy --workspace --all-targets ; echo $?
101                                        # aborts on the FIRST crate
```
With `--keep-going`, **10 crates fail to compile under clippy** (errors, not warnings):

```
crates/lunco-assets/build.rs:46:5              error: disallowed method `std::fs::write`
crates/lunco-modelica/build.rs:25:24           error: disallowed method `std::fs::read`
crates/lunco-sandbox-edit/build.rs:80,81,126   error: disallowed `fs::write` / `fs::read_to_string`
crates/lunco-doc-bevy/src/diagnostics.rs:48:33 error: disallowed `std::time::Instant::now`
crates/lunco-environment/src/horizon.rs:412,664 error: disallowed `std::time::Instant::now`
crates/lunco-twin/src/lib.rs:423:9             error: disallowed `std::fs::write`      (lib test)
crates/lunco-api/src/transports/assets.rs:149:9 error: disallowed `std::fs::write`     (lib test)
crates/lunco-materials/tests/materials_test.rs:40,58,85  error: disallowed `read_to_string`
crates/lunco-autopilot/src/lib.rs:544:27       error: approximate value of FRAC_PI_6  (PROBE_SPREAD = 0.5236)
crates/lunco-terrain-core/src/quadtree.rs:599,602 error: approximate value of FRAC_PI_4
crates/lunco-environment/src/horizon.rs:1187,1189,1208,1210  error: erasing_op (`bytes[0 * 8 + 3]`)
```

Two independent causes:
1. **`disallowed_methods = "deny"`** (root `Cargo.toml` + `clippy.toml`). `clippy.toml`'s own header
   says build scripts *"run at compile time on the host"* and `tests/` modules are **on the allow-list**
   — **but cargo has no path-scoped lint config. That allow-list exists only as prose.** Five of the ten
   failures are exactly the cases the comment claims are exempt.
2. **Default-deny correctness lints** (`approx_constant`, `erasing_op`) that **nobody has ever seen**,
   because clippy never gets that far.

⇒ `lunco-sandbox`, `lunco-usd-bevy`, `lunco-modelica`, `lunco-networking`, `lunco-workbench` — **the five
biggest crates — are never linted at all**, because their dependencies error out first. The 74 warnings
observed are a **floor, not a count.**

**Fix (11 one-line changes):** `#![allow(clippy::disallowed_methods)]` in the 3 `build.rs` and the
`lunco-materials` / `lunco-twin` / `lunco-api` test modules (as `clippy.toml` already claims);
`0.5236 → FRAC_PI_6`, `0.785… → FRAC_PI_4`; drop the `0 *` in the horizon tests. **Then wire clippy into
CI so it stays green.** This is the single highest-leverage hygiene fix in the repo.

### H2 · Clippy warning classes (partial — only the ~25 crates clippy could reach)

| n | lint | worst offender |
|---|---|---|
| 16 | `type_complexity` | `crates/lunco-environment/src/horizon.rs:348` |
| 8 | `too_many_arguments` | `crates/lunco-controller/src/lib.rs:86` (12/7), `crates/lunco-terrain-globe/src/quad_sphere.rs:41` (11/7) |
| 7 | `redundant_clone` | `crates/lunco-api/src/lib.rs:242:64` |
| 7 | `needless_borrow` | `crates/lunco-mobility/src/sensing.rs:243:82` |
| 4 | `erasing_op` (deny) | `crates/lunco-environment/src/horizon.rs:1187` |
| 4 | simplifiable `map_or` | `crates/lunco-environment/src/horizon.rs:1035:34` |
| 2 | `derivable_impls` | `crates/lunco-materials/src/dyn_params.rs:310` |
| 2 | manual assign-op | `crates/lunco-core/src/coords.rs:112` |
| 1 | `private_interfaces` | `crates/lunco-time/src/domain.rs:231` |
| 1 | `unnecessary_to_owned` | `crates/lunco-tools-rhai/src/lib.rs:145:47` |

`cargo build --workspace` is **clean of errors**, 60 warnings (14 unreachable-`pub`, 12 missing-docs, 3
unused `Result`, 4 never-read fields/consts, 1 unreachable pattern). `lunco-workbench` alone emits 27.

### H3 · Three dropped `Result`s on dock focus
`crates/lunco-workbench/src/lib.rs:707, 771, 1596` — `self.dock.set_active_tab(..)` result discarded.
Silent no-op if the tab path is stale; matches the "tab doesn't foreground" bug class.

### H4 · Poisoned-mutex `expect`s turn one panic into a permanent per-frame crash loop
`crates/lunco-doc-bevy/src/lib.rs:687,694,756,805,819` (×5, `"journal lock poisoned"`) and
`crates/lunco-usd/src/document.rs:642,650` (the **hot compose path**). One panic anywhere in a journal
writer poisons the mutex; **every subsequent access then panics, every frame, forever** — a single glitch
becomes an unrecoverable app.
**Fix:** `.unwrap_or_else(|e| e.into_inner())` — the journal has no invariant broken by a mid-write
panic — or switch to `parking_lot`.

### H5 · `panic!` on a user-supplied path at boot
`crates/lunco-sandbox/src/lib.rs:3151` — `TwinMode::Orphan(_) => panic!("expected folder or twin")` in
`load_startup_scene`. Reachable from a CLI/user-supplied twin path: **a directory that isn't a twin →
hard crash at boot** instead of an error toast.

### H6 · Other panic-surface notes
238 non-test `unwrap`/`expect`/`panic!` sites. **All 44 inside systems taking `Query<` were read**; most
are provably guarded (good discipline). The unguarded ones beyond H4/H5:
- `crates/lunco-scripting/src/world_bridge.rs:1160` — `.expect("engine Arc must be unique outside a task
  tick")`, reachable from script/REPL input; a re-entrant script call **panics the app** rather than
  erroring the script.
- `crates/lunco-networking/src/server.rs:130,162` — deliberate fail-loud `panic!` on half-set TLS env,
  documented as such — but they live in **lib** fns (`resolve_cert_paths`, `resolve_identity`), so any
  embedder inherits the panic. Low priority.
- **`todo!`/`unimplemented!`: zero in reachable code.** ✅

### H7 · Logging discipline (good, two leaks)
107 `info!`/`println!` inside fns taking `Query<…>` — the overwhelming majority are **fire-once event
handlers**, correct at `info!`. Genuine per-frame leaks:
- **`crates/lunco-avatar/src/lib.rs:1565-1567`** (`orbit_system`) — the `log_countdown` rate-limits the
  *converged* case, but the **`far_off` branch logs every single frame** during any orbital approach ⇒
  **60 lines/s of 200-char telemetry.** → `debug!`, or rate-limit that branch too.
- `crates/lunco-usd-bevy/src/lib.rs:3333,3336,3340` — developer breadcrumbs at `info!`. → `debug!`.
- `crates/lunco-celestial/src/trajectories.rs:305,342,353` — probe systems that exist *only* to log, at
  `info!`, ungated.

`println!`/`eprintln!` in library `src/`: only in `lunco-assets`' CLI path, where stdout **is** the UI —
correct. **Zero `dbg!` in the workspace.** ✅

### H8 · Docs that assert things which grep to zero
- `docs/usd-source-of-truth-ecs-projection-design.md:3` — **"Status: implemented"**; names
  `UsdPrimIndex`, `UsdAttrProjection`, `project_usd_attrs_to_components`. **All three: zero hits.**
- `docs/architecture/12-api.md:39,42,45,48,193,376` — documents `GET /api/health`,
  `GET /api/commands/schema`, `GET /api/entities`, `GET /api/entities/<ulid>`. **None exist.** The only
  registered route is `POST /api/commands` (`crates/lunco-api/src/transports/mod.rs:108`). **Every curl
  example in that doc 404s.**
- `docs/commands-reference.md` — header claims **153 commands**; source has **188** `#[Command]` sites.
  **`SetTerrainOverlay` does not appear at all.** It *does* document **`TestEcho`** — a unit-test fixture
  (`crates/lunco-api/src/executor.rs:739`) — as public API. Root cause: `tools/gen-command-docs`
  **text-scrapes `.rs` files** instead of consuming the runtime `DiscoverSchema` output (which is
  already derived, correct, and drives the MCP tool list).
  **Fix:** spin the headless app, dump `DiscoverSchema` JSON, generate from that; run it in CI and fail
  on diff.
- `docs/architecture/28-modelica-realtime-physics.md` — requires a realtime-safe promise *"declared in
  USD"* before a program may drive predicted physics. **Zero hits in code** (`A4`).
- `specs/005-multiplayer-core` FR-003 — claims rollback. **There is none** (`P12`).
- `docs/architecture/19-unified-time-and-clock.md:57` vs `:304` — the two lines contradict each other
  about whether Modelica advances with the world (`A3`).
- 24 commands ship with **no doc comment** — they render as `_(no description)_` in the MCP tool
  descriptions an agent reads.

### H9 · A failed command reports success
`crates/lunco-api/src/executor.rs:551-558` mints a `command_id` and returns `command_accepted` **before**
the command is deserialized or run. `:174-179` / `:190-192`: if the params don't deserialize, or the
type isn't constructible, the dispatcher `warn!`s and **drops** it. `QueryCommandResult` (`:616-624`)
then returns `outcome: null`, which its own doc says means *"either a bad id, or a fire-and-forget
command"* — **indistinguishable from success.**

⇒ `{"command":"SetPorts","params":{"target":"nope"}}` returns `200 OK`. **Every agent/MCP flow built on
this reports success on a typo'd param.**

Compounding: `crates/lunco-api/src/transports/http.rs:39` maps **every** error to **HTTP 500**, throwing
away the `CommandNotFound` (400) and `EntityNotFound` (404) codes **that are already typed** in
`schema.rs:44-49`. And errors are **strings** throughout (`CommandOutcome::Failed(String)`; handler sig
`Result<Ack, String>` — `crates/lunco-command-macro/src/lib.rs:246`): no error code, no field path,
nothing machine-actionable.

**Fix:** deserialize in `execute_request` (the registry is in hand there) and return
`ApiErrorCode::DeserializationError` **synchronously**; reserve `outcome: null` for "still pending"; add
an explicit `Dropped`/`Invalid` `CommandOutcome`; honour the error codes in the HTTP status.

### H10 · No transactionality; `ChangeSetId` is designed but never used
`crates/lunco-twin-journal/src/lib.rs:258` — `JournalEntry.change_set: Option<ChangeSetId>` is *"the
transaction-style undo unit"*. **Every recorder passes `None`**
(`crates/lunco-obstacle-field/src/journal.rs:59`; the USD auto-recorder records **one entry per
`UsdOp`**).

`AttachComponent` (`crates/lunco-usd/src/commands.rs:593`) lowers to **many** `UsdOp`s. Its own doc
concedes the design: *"undoable rather than silently half-applied behind a rollback the journal can't
see."* ⇒ if op 4 of 7 fails, ops 1–3 are journaled and applied, and one undo peels off **one** op.
**Fix:** wrap multi-op commands in a `ChangeSetId` at the handler; make the undo view group by it.

*(Done right here: the ops that **are** journaled record a lossless `(forward, inverse)` pair and only on
successful apply. A failed op is not journaled.)*

### H11 · Dead weight

> **⚠ CORRECTION (2026-07-13). Both claims in this section were wrong, and both were
> acted on before being checked. Read the resolution before trusting the text below.**
>
> - **`lunco-cache` was NOT "exactly the abstraction the codebase needed."** It was
>   deleted, not adopted. The "~8 bespoke memos" cited below are mostly a *different
>   shape*: of ~46 cache-like sites, ~17 are synchronous memos with no in-flight
>   window and ~20 are plain registries — `ResourceCache` would only add overhead. The
>   three sites named by name all fail: `modelica/class_cache.rs` is **synchronous**;
>   `horizon.rs` is a per-Entity Component, not a HashMap; `stream_viz.rs` needs
>   generation-versioning + inflight backpressure that `ResourceCache` lacks, so
>   migrating it would be a *downgrade*. Async dedup in this codebase is the ECS idiom
>   (a `Task` Component + `Without<BakeTask>` — the query filter IS the pending set),
>   which is why a `Resource<HashMap<K, Task>>` found no users. See
>   `docs/architecture/caching-and-precompute-strategy.md` §1.
> - **`lunco-terrain-globe` is NOT non-functional.** "Registers zero systems" is true
>   and irrelevant: it is the **geometry spine of the live orbital view** —
>   `globe_lod.rs` imports its cube-sphere math and `update_globe_lod` runs every
>   frame. A plugin with no systems is not a crate with no callers. Deleting it on the
>   strength of the paragraph below would have cost the orbital view.
>
> The real defects the cache audit *did* surface were all **invalidation**, not
> duplication: two material caches with divergent eviction, and a bitmap-texture memo
> that cached negatives and was invalidated by nothing. Fixed 2026-07-13.

- **4 orphan crates, 697 LOC, zero reverse-deps, zero references** (root `Cargo.toml:67` already names
  them): `lunco-attributes` (188), `lunco-cache` (229), `lunco-obc` (111), `lunco-telemetry` (169).
  **The painful one:** `lunco-cache` is a generic `ResourceLoader` + `ResourceCache<L>` **with in-flight
  dedup** — i.e. **exactly the abstraction the codebase needed and never adopted**, while ~8 bespoke
  ad-hoc `HashMap` memos proliferate (`spawn.rs:29`, `stream_viz.rs:390,459`, `horizon.rs:288`,
  `modelica/class_cache.rs`, `document/core.rs:67`, `package_tree/cache.rs:39`, `ui/widget.rs:106`).
  **Adopt it in modelica (whose caches match its load+dedup contract) or delete it.**
- **`crates/lunco-terrain-globe` (263 LOC) registers ZERO systems** — `TerrainPlugin::build` only calls
  `init_resource` + `register_type` (`lib.rs:73-80`). It is a **fully non-functional second LOD system**
  (`QuadSphere`, `TerrainTileConfig { max_lod: 12, max_tile_entities: 2000 }`) sitting next to the real
  one. The globe/surface transition is **not** implemented here. It reads like a live subsystem to anyone
  auditing. Delete it or mark it vestigial.
- **`tools/gen-command-docs/Cargo.toml` is not a workspace member and there is no `exclude`** ⇒ it is a
  separate workspace, **never built, linted, or tested** by any workspace command. Bit-rot candidate —
  and it's the thing generating the stale docs in `H8`.
- **`crates/lunco-celestial`'s `ureq` dep** (`Cargo.toml:56`) — **zero references** in its `src/`. The
  ephemeris-download path it was added for is gone.
- **`crates/lunco-avatar/src/recording.rs:57`** — `RecordingSettings::overwrite` is dead (the relocator
  that consumed it was deleted); the doc calls it *"advisory"*, which is a back-compat shim by another
  name. **Also `PLAUSIBLE` (`:338-345`):** nothing calls `create_dir_all` on the resolved `output_dir` any
  more (the deleted `relocate()` did) — if `~/Videos` doesn't exist, the encoder likely fails to open the
  file at stop **with no user-visible error.**
- `crates/lunco-autopilot/Cargo.toml:20` — `quick-xml = "0.39"` is the only non-`workspace = true` dep in
  that manifest. Hoist to `[workspace.dependencies]`.
- Only **8 `#[allow(dead_code)]`** workspace-wide and **zero `#[deprecated]`** — unusually clean.

### H12 · TODO/FIXME markers that flag known-broken behaviour (not aspirations)
- `crates/lunco-modelica/src/lib.rs:1652` — `FIXME: collect_stepper_observables was removed during the
  …` — a capability was deleted and **the call site left dangling.**
- `crates/lunco-modelica/src/worker_transport.rs:588` — *"… Modelica compile/run is BROKEN until it is
  rebuilt"* — a runtime-detectable broken state shipped as an error string.
- `crates/lunco-modelica/src/worker_transport.rs:1240` — `TODO(CQ-213)`: the **165 MB deep clone**.
- `crates/lunco-modelica/src/bin/lunica_worker.rs:26` — `TODO(arch-msl-handoff)`: the worker can't see
  MSL from its own process.
- `crates/lunco-obstacle-field/src/plugin.rs:190-196` — `TODO: remove Standalone` (see `R6`).

### H13 · Test coverage is inverted — the biggest crates are the least tested

| crate | LOC | tests | |
|---|---|---|---|
| **lunco-sandbox** | **6854** | **2** | the app crate: **1 test / 3.4k LOC** |
| lunco-workbench | 12094 | 27 | 1 / 450 LOC |
| lunco-terrain-surface | 7722 | 20 | |
| lunco-environment | 2054 | 5 | **and its test target doesn't compile under clippy** (`H1`) |
| lunco-viz | 2082 | 4 | |
| lunco-doc-bevy | 1091 | 1 | |
| lunco-theme / tutorial / command-macro / settings / hardware / terrain-globe / worker-transport / luncosim / render / web / robotics | 969 … 51 | **0** | |

**Well-tested by contrast:** `lunco-terrain-core` (99 tests / 4.3k), `lunco-usd` (126 / 8.3k),
`lunco-modelica` (400 / 69k + 20 integration files).

**New modules in this branch that ship with zero tests:** `terrain-surface/overlay.rs`. (With tests:
`terrain-core/field.rs`, `transfer.rs`, `autopilot/btcpp_xml.rs` — but see `D3`#13, they're happy-path
only.)

**Integration tests DO run headless** — `MinimalPlugins`/`HeadlessPlugins` + `BigSpaceDefaultPlugins`
across ~70 files in `crates/*/tests/`; no `DefaultPlugins`/GPU dependency. ✅ The 15 `#[ignore]`s are all
in `lunco-modelica` with honest reasons pinned to upstream rumoca bugs — **that's how it should be
done.**

---

# 8. WHAT IS GENUINELY WELL BUILT

Not filler. Each of these is something a reviewer went looking to indict and **could not** — and several
are better than the median commercial equivalent. **Do not "simplify" these away.**

- **Time.** `crates/lunco-time/` — TDB is the master clock; UTC/TAI/TT/UT1 are *derived* through a real
  leap-second table + the fixed 32.184 s + the periodic TDB−TT term. `epoch = epoch0 + (tick − tick0)/86400`
  is a **pure function of an integer tick** — no `epoch += dt` accumulation anywhere, so it is seekable
  and drift-free. `utc_now_tdb_jd()` fixed the classic 69 s "treat `Utc::now()` as a JD" bug and there is
  a test asserting TDB−UTC = 64.184 s. **Pause is a *flag*, never `relative_speed = 0`** (because
  lightyear's interpolation divides by it), locked in by
  `frozen_spine_pauses_and_never_zeroes_relative_speed` (`lib.rs:555`). DUT1 = 0 is documented with its
  ~15″ GMST consequence. **No code treats UTC as uniform.** Most sims of this class get this wrong.
- **Physics is fixed-step**, from **one** constant (`FIXED_HZ`, `crates/lunco-core/src/lib.rs:445`); Avian
  steps in `FixedPostUpdate`; `SimTick` advances once per fixed step. **No render dt leaks into
  integration anywhere.**
- **The big_space cell-edge doctrine** (`crates/lunco-celestial/src/big_space_setup.rs:192-216` +
  `tests/grid_cell_edge_precision.rs`) correctly identifies that `LocalFloatingOrigin::translation` is
  f32 bounded by `edge/2 + threshold`, that the **coarsest** grid sets the precision floor for its whole
  subtree, and that cells are i64 so small edges are free. Every celestial grid is 2 km / 100 m ⇒ **ULP ≈
  0.12 mm.** Most projects reach for 1e9 m cells and never understand why the ground jitters. (`P1` is
  the one place this doctrine wasn't applied.)
- **f64→f32 narrowing happens AFTER the camera-relative subtraction, everywhere checked** — tile meshes
  (`terrain-globe/tile.rs:36`), the bridge writeback (`big_space_bridge.rs:327-344`, which subtracts the
  parent frame **and** the cell in f64 before `.as_vec3()`). **Avian is f64 end-to-end** (`f64`,
  `parry-f64`) and the bridge **severs avian's f32 `GlobalTransform` sync wholesale** rather than
  round-tripping. That is the rule, and it is followed.
- **Depth precision at planetary scale is handled by construction, not luck.**
  `crates/lunco-avatar/src/lib.rs:2530-2598` derives `near` from the nearest celestial body surface and
  `far` from the farthest, per frame. With reverse-Z: on the surface, a 1 cm separation at 1 km gets
  **~150 ULP**. In orbital view, anchoring `near` to the viewed body puts it where reverse-Z precision
  peaks — **that is what you do instead of a log-depth buffer.** No multi-frustum needed.
- **The CDLOD core is 3D-Tiles-grade.** Error-driven selection with **measured** node error
  (`quadtree.rs:184-193`) memoized against the oracle pointer — not the usual `root/2^depth` schedule, so
  crater rims earn depth and flat mare stays coarse. A **canonical** screen metric
  (`CANON_SCREEN_H_PX`/`CANON_FOV_Y_RAD`) makes selection resolution-independent and **peer-identical**.
  And the non-obvious one: **the tile budget is enforced by coarsening the metric, not by capping the
  walk** (`stream_viz.rs:866-881`) — a hard cap would leave the morph bands assuming unbudgeted refine
  distances and produce a visible LOD line. People get that wrong for years.
- **The idle-camera signature gate** (`stream_viz.rs:780-817`): quantised focus + eye height + rover
  footprints + oracle identity + LOD knobs folded into one FNV hash; skips the entire selection when
  nothing moved. Textbook change-driven — and the "prefer lazy systems" discipline is **real across the
  codebase**, not aspirational (`scatter_terrain_layers` gated `Without<TerrainLayersApplied>`,
  `sync_terrain_overlay` on `is_changed()`, `bind_derived_maps_to_tiles` on `Changed<>`,
  `despawn_orphaned_lod_tiles` on `RemovedComponents`, …).
- **Geomorph + reveal-settle** so both LOD switches *and* live height re-bakes are pop-free, with
  stale-generation tiles that keep covering the surface until their replacement bakes in (`TileSlot::gen`,
  `reap_stale`) — progressive regeneration instead of a despawn flash.
- **Procedural rock batching**: 6 shared bucket meshes + 1 shared material ⇒ ~6 draws for 6000 rocks,
  `NotShadowCaster`, `VisibilityRange`-culled, web-skipped with a clear rationale. Tiles are
  `NotShadowCaster` with a documented ~16 ms saving. `horizon_march.wgsl:57-66` has proper early-outs, a
  bounded 48 iterations, and geometric step growth — WebGL2-safe.
- **Netcode identity is connection-bound, never wire-supplied** (`server.rs:870-891`) — **the single most
  important thing to get right in a server-authoritative design, and it is right.** Directional authority
  guards (host refuses inbound `Snapshot`/`Spawn`/`Despawn`, and rejects a `Spawn` for an existing gid)
  are all present.
- **The content plane is properly hardened**: chunk admission gated on the manifest-advertised size
  **before any buffer opens**, `offset == buf.len()`, incremental hashing, fail-closed CID verification,
  `safe_rel_path` traversal guard, `MAX_ASSET_OFFER_BYTES` cap. Decode is bounded by a 16 MiB frame cap
  **and** `bincode::with_limit`, so a lying length prefix can't pre-allocate.
- **AOI** with enter/exit hysteresis, owned + predicted-Dynamic force-include, soft-exit re-baseline, and
  a **pure, unit-tested decision core.** Snapshots capped to one lightyear fragment with correct
  `(1-p)^n` reasoning. **Cell-aware quantization** (cell + mm remainder) avoids i32 saturation at orbital
  range, with a test proving the old absolute form failed.
- **The journal double-apply fix is general, not a call-site patch** — replay is selected by
  `domain_ops_after` in convergent `merged_order_ids` order, filtered by base head + `already` set +
  author; `append_remote` dedups by `EntryId`. **Idempotent by construction**, and domain-parameterized
  rather than USD-special-cased.
- **Reflection-derived command schema.** `#[Command]` emits `Event + Reflect + reflect(Event) +
  Serialize/Deserialize` in one attribute; `DiscoverSchema` walks the type registry; **the MCP server
  generates its tool list from that at runtime.** Zero hand-maintained tool defs. `#[sync_local]` /
  `#[authz_target]` as reflect attributes mean **authorization keys off the *type*, not a field-name
  heuristic** — with tests (`executor.rs:861-946`) proving the old heuristic bug is gone. Rare and right.
- **Rhai is one bridge, not two** — `bridge_core.rs` (language-neutral) + `world_bridge.rs` (marshalling
  only). `cmd()` routes through the **same** `ApiCommandEvent` as HTTP/MCP. Replay-safe seeded RNG
  (`rng_begin(gid, tick, salt)` + SplitMix64). The client-scoped sandbox (`bridge_core.rs:438-503`) is
  deny-all with an ownership-gated escape that **reuses the same `CommandPolicyRegistry`** rather than a
  second allowlist, and rejects are deduped into authoring diagnostics instead of per-tick log spam.
- **9 engine-free leaf crates** (terrain-core, terrain-bake, twin-journal, storage, doc, hash,
  worker-transport, behavior, precompute) with **no `bevy` dep at all**. **Zero Cargo cycles.** Excellent,
  testable, wasm-portable spine.
- **Picking is genuinely ONE pipeline** (a rival `ScenePointer` path was found and removed — recorded at
  `selection.rs:192`). **Panels are a trait-object registry.** **Materials are WGSL-reflected, not a
  hardcoded table.** Terrain field→derive→transfer is a real staged split. Contact-force sensing reads
  `warm_start_normal_impulse` rather than the naive `total_impulse/dt` — someone actually read the
  solver's semantics. Batch/experiment runs correctly treat `dt` as an **output interval, not an
  integration step**, and run faster-than-real-time.

**The discipline is real. What is missing is enforcement** — CI that runs clippy, a `cargo tree` guard on
the headless builds, a doc generator fed from the runtime schema, and a deny-by-default wire gate.

---

# 9. FIX ORDER

### Week 1 — security (all remote-reachable)
| # | id | change |
|---|---|---|
| 1 | `S1` | allowlist inbound command types to `SyncChannelRegistry` in `apply_sync_command` — **one `if`** |
| 2 | `S2` | real netcode key; refuse a non-loopback bind while the dev key is in use |
| 3 | `S3` | one `sandboxed_engine()` for all 5 rhai `Engine::new()` sites |
| 4 | `S4` | gate `set_component_field` / `set_resource_field` behind `enforce_script_authority` |
| 5 | `S6` | path-confine `CaptureScreenshot` + the `Open*` commands; clamp `SpawnDemTerrain.target_res` |
| 6 | `S5`,`S7` | server-side role assignment; `JOURNAL_EDIT` → `Operator`; validate `entry.id.author` |

### Week 1 — one-liners with outsized payoff
| # | id | change |
|---|---|---|
| 7 | **`A1`** | `default-features = false` in `crates/lunco-workspace/Cargo.toml:14` + the CI `cargo tree` guard. **The highest-leverage character in this repo is a comma.** |
| 8 | `P1` | `switching_threshold: 100.0` in `crates/lunco-core/src/world.rs:75` |
| 9 | `R3` | `RenderAssetUsages::RENDER_WORLD` for the LOD tile path |
| 10 | `R4` | `Msaa::Off` on wasm; `hdr: true` or delete the 4 `Bloom` configs |
| 11 | `R2` | evict `LodMeshCache` in `despawn_orphaned_lod_tiles` |
| 12 | `R6` | `ObstacleFieldMode::default() = DemDelegated` (or delete `Standalone`) |
| 13 | `A5` | remove `Step` from `is_squashable` |
| 14 | `A11` | fix the duplicate `"back"` match arm |
| 15 | **`H1`** | unbreak clippy (11 one-line fixes) — **then wire it into CI**, or every finding here re-accumulates |

### Next — correctness users will hit
16. `N1` — scope/reset `AppliedInputSeq` (silent permanent prediction death in normal play).
17. `D1`, `D2` — overlay `enabled` sentinel; overlay must read the field, not the LOD normal.
18. `D3` — BT XML codec: match on the enum; **add the `patrol` test that fails today**.
19. `D6` — pick gate: latch ownership on press, fix `dock_rect`, gate `record_chrome_panel` on
    `transparent_background()`, split `PanelRects`.
20. `R10`, `R11`, `R12` — bake-pool respawn; non-finite carve guard; real `target_pixel_error` clamp.
21. `P2`, `P3` — IAU W₀ + Kepler tilt. **The Moon's near side currently does not face Earth.**
22. `R1`, `R5`, `R7` — wasm main-thread bakes; reveal material clones; LOD hysteresis.
23. `D5`, `D7`, `D8`, `D9` — horizon re-bake re-arm; `set_if_neq`; overlay material filter; web base grid.

### Then — architecture
24. **`A3`/`A4`** — the co-sim macro step + the tier gate. **This is the one that makes results wrong in a
    way nobody notices.**
25. `A2` — wire the gizmo to `MoveEntity`; `A8` — delete `undo.rs`; `A6` — move 2 types to `lunco-core`.
26. `A7` — reflect-ify the inspector (−1400 LOC); `A10` — `inventory`-ify `instantiate_prim` + the
    usd-sim API-schema probes; merge the two wheel tables.
27. `N2`, `N3`, `N4`, `N5` — input drain on the fixed clock; a desync digest; a wire-version handshake;
    chunked journal replay.
28. **Decide `A9`/`P12`:** either build the command journal + true replay, **or delete the claims from
    the docs and specs.** Pick one story and make the docs true (`H8`).
29. `P8` — frame newtypes. Makes `P2`/`P3`/`P6`/`P7` structurally unrepresentable.
30. `H11` — delete the 4 orphan crates + `lunco-terrain-globe`, or adopt `lunco-cache` in modelica.
    Bring `tools/gen-command-docs` into the workspace and feed it from `DiscoverSchema`.

---

# Appendix — repro commands

```bash
# scope of the branch
git diff --stat main...HEAD
git log --oneline main..HEAD

# H1 — the clippy gate is dead
cargo clippy --workspace --all-targets ; echo "EXIT=$?"     # → 101, aborts on first crate
cargo clippy --workspace --all-targets --keep-going 2>&1 | grep '^error'

# A1 — egui in the headless build
cargo tree -p lunco-sandbox-server -i bevy_egui             # → should FAIL after the fix
cargo tree -p lunco-sandbox-server | wc -l                  # 840 (GUI: 913)

# A2 — the gizmo never touches USD
grep -c 'usd\|Usd' crates/lunco-sandbox-edit/src/gizmo.rs   # → 0

# A4 — the tier contract does not exist
grep -rn 'tier\|Tier' crates/lunco-cosim/ crates/lunco-usd/ # → no coupling-tier hits

# P5 — no light-time
grep -rn 'light_time\|aberration\|speed_of_light\|299792' crates/   # → 0

# N3 — no desync detector
grep -rn 'checksum\|state_hash\|desync\|crc' crates/lunco-networking/

# R4 — MSAA is never configured
grep -rn 'Msaa' crates/                                     # → 0

# H8 — docs assert what doesn't exist
grep -rn 'UsdPrimIndex\|UsdAttrProjection\|project_usd_attrs_to_components' crates/  # → 0
grep -rn 'api/health\|api/entities' crates/lunco-api/src/transports/  # → 0
grep -rn '#\[Command' crates/ | wc -l                       # 188 (doc claims 153)

# H11 — orphan crates
for c in lunco-attributes lunco-cache lunco-obc lunco-telemetry; do
  echo "$c: $(grep -rl "${c//-/_}::" crates/ | grep -v "crates/$c/" | wc -l) external refs"
done
```
