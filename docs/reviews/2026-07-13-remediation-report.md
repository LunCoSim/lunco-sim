# Remediation report — 2026-07-12 review + render decoupling

**What this is:** the closing report for the work done on 2026-07-12/13 against
[`2026-07-12-full-code-review.md`](2026-07-12-full-code-review.md), plus the render-decoupling refactor
that followed it.

**Bottom line. Every finding in the review is fixed except one set, deliberately:**

| deferred | why |
|---|---|
| **§1 Security (`S1`,`S2`,`S4`,`S5`,`S7`, the path half of `S6`)** | The project **accepts that it does not enforce access control.** **Trusted LAN only — never expose a host to an untrusted network.** Recorded in [`TODO-rbac-not-enforced.md`](TODO-rbac-not-enforced.md) with the six-step path to enforcing it. |

Also not done, and honest about it: **`P8`** (frame newtypes — the review's highest-leverage remaining
fix; wants its own pass), and the **rumoca conditional-algebraic bug** (`m_dot` reads 0 at full throttle —
upstream, in someone else's solver; marked `TODO(rumoca-observables)`, not improvised around).

**Verification: 1625 tests pass, 0 fail. `cargo clippy --workspace --all-targets` exits 0. The `--no-ui`
server links no `wgpu`, `bevy_render`, `bevy_pbr`, `bevy_core_pipeline`, `bevy_gizmos`, `egui` or
`winit`.** (Baseline at session start: 1488 tests, clippy aborting on crate #1, server linking the lot.)

---

## 1. The three findings that mattered most

### Clippy was pointed at the wrong target — that is *why* it had never run

`cargo clippy --workspace --all-targets` exited **101**: it aborts on the first crate that fails, and ten
crates failed to *compile* under it. The five biggest crates — `lunco-sandbox`, `lunco-usd-bevy`,
`lunco-modelica`, `lunco-networking`, `lunco-workbench` — had **never been linted at all**, and the
warning count anyone had ever seen was a floor, not a count.

The fix was not a backlog grind. It was a **targeting error**:

> The wasm-portability bans (`std::fs`, `std::thread::spawn`, `std::time::Instant::now`) were being
> enforced **on native — the one platform where they cannot be true.**

- `std::fs`/`thread` call sites are mostly already inside `#[cfg(not(target_arch = "wasm32"))]`, where
  using them is *correct*. Native clippy sees that code anyway and flags it.
- Worse, `web_time` supplies its native impl via `pub use std::time::*`, so on native
  `web_time::Instant` **is** `std::time::Instant` — the same DefId. Clippy resolved straight through the
  re-export and flagged every *correct* caller: **73 hits in `lunco-modelica` alone, all false, not one
  true.**

**A lint that is wrong every single time it fires is a lint people silence.** That is precisely how the
hole stayed open. Those bans now run on `--target wasm32-unknown-unknown`, where `cfg` strips
native-only code for free and the two `Instant` types are genuinely distinct — so they fire **only** on
code that will actually break in a browser.

**The new gate paid for itself on its first run**, catching two things no native build or lint can see:

1. **The web build was broken.** `ApiResponseEnvelope` gained an `error_code` field; `transports/wasm.rs`
   — which only compiles on wasm — was never updated.
2. **`indexer.rs` shipped `std::time::Instant` into the browser**, where `Instant::now()` **panics**. It
   is an unconditional `pub mod`.

It also named **16 genuinely wasm-reachable `std::fs` calls** in `lunco-modelica` (the MSL indexer, the
package browser, the icon loader) — which is the long-standing *"MSL missing on the web"* symptom,
finally localized. These are **not** `#[allow]`ed away: a non-fatal CI step prints the count and the
sites every run, and it must trend to zero.

### The co-simulation was paced by the render frame

`Step` was dispatched to a worker thread once per fixed tick, but a per-model `is_stepping` flag meant
the 2nd..Nth fixed ticks *inside one render frame* were **skipped**. Net effect:

> **The Modelica model advanced at most once per RENDER FRAME while Avian advanced once per FIXED step.**

At 30 FPS the model ran at **half speed**. At `rate = 10`, ten times too slow. **Change your GPU load,
change the physics answer.** And `dt` was always `Time<Fixed>::delta`, never the real gap, so skipped
macro-steps were lost model time *permanently*.

Now: a real macro-step contract. `target_time` advances by exactly one fixed delta per unpaused tick;
`plan_macro_step` requests `dt = target − current` (clamped), so a model that fell behind — a long
compile, a hitched frame, a rate burst — **catches the time up** instead of losing it. A new `CosimLag`
resource measures `|model − world|` every tick and warns past 0.25 s; **nothing measured this before.**
Tests assert model time equals world time after 600 ticks *at any worker latency* (0, 1, 2, 5, 10 ticks).

`Step` was also **squashable** — two queued steps collapsed, the earlier one dropped, and a **fake
success** returned for it. Squashing is right for an idempotent setpoint; `Step` is an *integration*.
Removed.

### The gizmo — the primary edit path — never wrote USD

`grep -c 'usd' gizmo.rs` → **0**. Drags wrote `Transform` directly, so **every move was lost on reload**.
The correct path (`persist_move_to_runtime_layer`, which authors `UsdOp::SetTranslate`) *existed* and was
only ever fired from tests. It is now fired on drag-end. `undo.rs` — a private, in-memory, un-journalled
second history — is deleted; Ctrl+Z goes through the Twin journal. Along the way: **no `UndoDocument`
observer existed for USD documents at all**, so undo on a USD twin was a **silent no-op**.

---

## 2. The render decoupling

**Result:** the `--no-ui` server links **no wgpu, no bevy_render, no bevy_pbr, no bevy_core_pipeline, no
egui, no winit.** Design and enforcement: [`render-decoupling.md`](../architecture/render-decoupling.md).
Shader parameters and texture layers: [`shader-layers-and-params.md`](../architecture/shader-layers-and-params.md).

The rule turned out to be one line — *a domain crate may name `Mesh3d`; it may not name
`MeshMaterial3d`* — and **it needed no `#[cfg]` in the simulation.** Domain crates state appearance as
intent (`PbrLook` / `ShaderLook` / `SceneCamera` / `WorldLabel`); `lunco-render-bevy` is the only crate
that binds it; headless simply never adds that plugin.

**But the decoupling's real value was what it exposed.** Five bugs that no test would have caught:

1. **An unbounded material leak.** USD animates `displayColor` via timeSamples. A content-keyed material
   cache re-keys *every frame* under animation — minting a material per frame and freeing none. Presents
   as a slow memory climb, not a crash. Closed with an explicit `unshared` opt-out.
2. **Shared-material bleed.** With materials shared by content, the Inspector's `Assets::get_mut(handle)`
   would have recoloured **every entity that looked alike** — edit one rock, recolour all of them.
3. **`solar` was gated behind `#[cfg(feature = "render")]`** despite naming nothing render — so a
   **sun-tracking Modelica model running headless was silently receiving nothing at all.**
4. **A dead duplicate `CaptureScreenshot`.** `execute_request` matches the command by name and returns
   early, so the `#[Command]` + observer was **unreachable**; every screenshot came from `lunco-api`'s own
   spawn. And the registered command was `CaptureScreenshot {}` — **no fields** — while the real one took
   `save_to_file`/`path`/`region`. **The schema that generates the MCP tool list was lying.** Now one
   command, in the crate that implements it, declaring its real parameters.
5. **`R4`** (which nobody owned): **MSAA was never configured anywhere** in the workspace, so Bevy's
   default `Sample4` was silently on — including WebGL2, where 4× multisampling on a full-screen terrain
   is the most expensive default in the build. And **bloom was attached to non-HDR cameras in four
   crates**, where it renders nothing and still pays for a downsample/upsample chain. `SceneCamera` now
   *refuses* bloom without HDR and warns — the bug is unrepresentable rather than merely fixed.

**The last edge is the lesson.** After every material, camera, and shader had been decoupled, the thing
still linking wgpu into the server was **`lunco-celestial` depending on `lunco-api` without
`default-features = false`** — and `lunco-api`'s defaults include `render` (the screenshot readback).

That is *exactly* finding `A1` — the missing comma that opened this whole review — **hiding one layer
deeper.** The same trap, twice, in the same codebase, invisible both times. And the edge before *that*
one was a single **billboard text label** on a spacecraft (`bevy_sprite_render` pulls `bevy_render`).

Nobody would guess the server links a GPU driver because of a text label. **Only `cargo tree` sees it.**
Hence the `render-decoupling` CI job. **Do not delete it.**

---

## 3. Behaviour changes worth knowing about

Everything below is deliberate. Nothing else should be visible.

- **`MAX_REALTIME_RATE` 100 → 8.** At rate 100 a hitched frame demanded ~198 fixed steps (≈2376 avian
  substeps) in one frame, which made that frame slow, which demanded the same burst again — a guaranteed
  death spiral. **Rates above 8 now fall into `KinematicWarp`** (tick frozen; ephemeris/lighting still
  advance) instead of trying and failing to integrate physics. If a scenario ran physics at 20×, it will
  now warp.
- **Bloom stays off.** It was already a no-op (no HDR target anywhere), so today's output is preserved
  byte-for-byte — but the binder now *warns* instead of silently paying for the passes. Enabling it for
  real is a separate, deliberate call: `SceneCamera::with_bloom()` turns HDR on for you.
- **MSAA is now explicit**: `Off` on wasm, `2×` native (was Bevy's unset `Sample4` everywhere).
- **`ObstacleFieldMode::default()` is now `DemDelegated`** — the cheap path. `Standalone` (the 43×-FPS
  landmine) is opt-in. A default that is the pathological path is a fuse, not a fix.
- **`SetTerrainOverlay` fields are `Option<T>`.** `{"cliff_deg": 25}` used to *silently disable the
  overlay*, because `enabled` was written unconditionally from a `#[Command(default)]` false.
- **Material binding is now an observer**, so it lands a frame-boundary after the spawn. Anything that
  depended on the material existing in the same tick as the mesh was a latent ordering bug.
- **Diagram edge arrowheads on causal edges are gone.** They were already disabled by a hardcoded
  `if false && …` (clippy's `overly_complex_bool_expr` found it once it could finally run). I removed the
  dead branch — **re-enabling them is a visual decision for you**, not one I would make unilaterally.

---

## 4. Known-broken, honestly flagged

- **`rocket_engine_observables_round_trip` is `#[ignore]`d, and the bug is real.** Re-enabling the test
  that had been disabled behind a bogus FIXME proves **rumoca's elimination reconstructor evaluates
  conditional algebraics as 0.** In `RocketEngine.mo`, `m_dot` at full throttle reads **zero**, taking
  `thrust` and `p_chamber` with it — **every algebraic observable behind an `if` is dead.** The model's
  own comment says the Boolean was inlined *specifically* to dodge this; the workaround does not work.
  Marked `TODO(rumoca-observables)`. This is an upstream fix and I did not improvise one.
- **`naga` is still linked headless**, via `bevy_shader` — the WGSL *compiler*, kept so a shader edit
  renders without a disk round-trip. A compiler, not a GPU stack.
- **16 wasm-unsafe `std::fs` calls in `lunco-modelica`** — the "MSL on the web" debt, now counted in CI.
- **`commands-reference.md` was not regenerated.** The generator now consumes `DiscoverSchema` instead of
  text-scraping `.rs` files (it used to document `TestEcho`, a unit-test fixture, as public API), but
  running it needs a live app: `cargo run -p lunco-sandbox-server -- --api --no-ui &` →
  `curl -s 127.0.0.1:4101/api/commands/schema > /tmp/schema.json` →
  `cargo run -p gen-command-docs -- --schema /tmp/schema.json`. That run also prints the list of
  commands still missing doc comments.
- **`A7` (reflect-ify the Inspector, −1400 LOC) was skipped**, correctly: a 2261-line rewrite that could
  not be compile-verified at the time. A hardcoded inspector beats a broken one.

---

## 4b. Celestial, netcode, USD — the second wave

**The Moon's near side now faces Earth.** `lunco-celestial/src/iau.rs` is the IAU/WGCCRE rotation model,
authored **once** as the published ICRF elements (`α₀`, `δ₀`, `W₀`, `Ẇ`, plus the lunar periodic series).
The pole, the body-fixed rotation and the spin rate are all *derived* — there is no second copy to drift,
which is exactly how the bug happened (a hand-typed "mean-of-2026" pole sitting beside a rotation model
with no phase at all).

**The frame trap, stated so nobody repeats it.** `W₀` is published east of the **node of the body equator
on the ICRF equator** — *not* east of this engine's ecliptic +X. It can never be pasted in as a spin
angle. The model transforms pole *and* node into the engine frame and composes, so the node carries the
reference-direction offset and **no per-body fudge exists**. Earth's prime meridian lands at RA **280.147°**
(= GMST at J2000), not the 190.147° a naive reading gives; `earth_prime_meridian_is_at_ra_280_deg_not_190`
fails loudly if anyone re-pastes it.

**And the part nobody had noticed:** the Moon's *mean* α₀/δ₀ put its pole within **0.02° of the ecliptic
pole** — the entire 1.54° Cassini tilt lives in the periodic terms, because the pole precesses on an
18.6-year cone and the mean averages it away. Using mean elements alone would have **silently flattened
the lunar obliquity and destroyed the ±2° polar solar season.** The series is not optional;
`moon_pole_has_the_cassini_tilt` proves it.

Also: an i = 90° "polar" orbit used to top out at **66.6°** of latitude (`P3`); the orbit camera was
parented to a *rotating* grid, so the "star-fixed" view spun at 1 rev/sidereal-day (`P4`); big_space cell
binning was **disabled** (`switching_threshold: 1e10` ⇒ everything in cell 0 with raw f32 ⇒ 32 m ULP at
Earth–Moon distance) (`P1`); light-time is now published (`P5`); solar azimuth was **south-referenced**,
so every Modelica sun-tracker got an angle 180° from what it expected (`P6`).

**Netcode.** A re-possessed vehicle's prediction was **never reconciled again, ever** — normal gameplay,
no attacker: the ack watermark was keyed by gid alone and never cleared, so the host kept stamping the
*previous* owner's seq (`N1`). The host also acked `max(seq)` at *receive* time, **claiming to have
applied inputs it never integrated** (`N2`); the ack is now the seq the fixed tick actually consumed.
Plus: a per-body divergence gauge (silent 6 m snaps are now counted, announced rebaselines), a
wire-version handshake, chunked+budgeted connect-time journal replay, a bounded AOI fail-open, and the
netcode→editor dependency edge cut (`A6`).

**USD.** An Omniverse/Isaac stage — **Z-up, centimetres, *their* defaults** — imported **rotated 90° and
100× too small, silently.** `metersPerUnit`/`upAxis` are now honoured, converted **once, at the importer**
and baked into the shared decoders (a root-entity rotation is what doc 41 explicitly rejects). Multi-op
commands (`AttachComponent`) now undo as one unit.

## 5. Closing remarks

The review's thesis was *"the engineering craft is high; the enforcement is absent."* Fixing it confirmed
that from an angle I did not expect: **almost every bug found here was invisible to the people who wrote
the correct code right next to it.**

- The co-sim ordering was documented precisely and correctly on paper — and the code was paced by the
  render frame anyway.
- The USD-authority design was real, well-built, and bypassed by the primary edit path, while the design
  doc said *"Status: implemented"* for three types that grep to zero.
- The material-sharing discipline was correct in one rock path and forgotten in the other.
- The wasm bans were well-reasoned, well-commented, and aimed at the wrong target — which is why they
  produced 73 false positives and nobody ever fixed one.
- And a single missing `default-features = false` re-linked a GPU stack into the headless server. **Twice.**

The common thread is that none of these are failures of skill or care. They are failures of *feedback*.
Each one was a place where the codebase could not tell you that you were wrong. So the durable output of
this work is not the ~60 fixes — it is the four gates that will say so next time:

1. **clippy, native** — now actually runs, on every crate.
2. **clippy, wasm32** — where the portability bans are true, and which found a broken web build on day one.
3. **`cargo tree` render guard** — because feature unification is invisible to code review, and only the
   dependency graph knows.
4. **`DiscoverSchema`-generated command docs** — so the API surface cannot drift from what is registered.

Keep those four green and this document should not need a sequel.

The three things I would do next, in order: **`N1`** (`AppliedInputSeq` silently kills prediction on any
re-possessed vehicle — normal gameplay, no attacker needed), **`P2`** (the Moon's near side does not face
Earth), and a decision on **RBAC** — either enforce it or keep the honest note in
[`TODO-rbac-not-enforced.md`](TODO-rbac-not-enforced.md) and never expose a host.
