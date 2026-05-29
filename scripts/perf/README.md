# Performance Profiling & Optimization Subsystem

The durable toolchain + rules for keeping LunCoSim's frame loop fast. Our
binaries run a real-time 3D scene + Avian physics + an embedded egui IDE on one
frame loop, so a regression in *any* domain crate shows up as dropped FPS. This
subsystem is how you find which one — by **measurement**, never by guessing.

> Rule zero: **measure before you optimize.** A code-review "this looks hot"
> instinct is wrong more often than right. The 2026-05-29 sandbox regression
> (47 FPS on an RTX 5060) looked like a per-frame transform-propagation loop;
> profiling proved that cost ~0 ms and the real culprit was a USD cosim system
> deep-cloning the whole stage every frame. A/B-disable confirmed it; the
> flamegraph located it. Do that, in that order.

## Toolkit (this folder)

| Tool | What it does |
|------|--------------|
| `profile.sh` | One command: build → run under [`samply`](https://github.com/mstange/samply) with vsync off → auto-stop → print adapter, frame time, and symbolicated hot functions. Entry point. |
| `symbolicate_samply.py` | Resolves a samply `--save-only` capture (raw addresses) to function names via `addr2line`, aggregates self-time across all threads, supports `--skip-start SECONDS` to drop scene-load noise. |
| `parse_samply.py` | Quick raw self/inclusive tables straight from the capture (no symbolication) — a fast sanity view. |
| `captures/` | Output dir for captures + app logs. Git-ignored — never commit a capture. |

## Usage

```sh
# Full per-function profile of a release-codegen build (most representative):
scripts/perf/profile.sh --release

# Profile a different binary, longer window, custom scene:
scripts/perf/profile.sh --bin lunica --release --duration 30 --scene path/to.usda

# Reuse the last build; quick frame-time-only run (no profiler, no sudo):
scripts/perf/profile.sh --release --no-build --diag-only

# Re-analyze an existing capture, steady-state only:
scripts/perf/symbolicate_samply.py scripts/perf/captures/<file>.json.gz 40 --skip-start 10
```

**Commit the fix and a one-line before/after in the PR — never the multi-MB
capture.**

## Setup (one-time)

- `cargo install samply` (already present on the dev box).
- samply needs kernel sampling access for a non-root user:
  ```sh
  echo 1 | sudo tee /proc/sys/kernel/perf_event_paranoid   # runtime-only, resets on reboot
  ```
  Without it, `profile.sh` auto-degrades to `--diag-only` (frame time + GPU
  adapter, but no per-function attribution).
- The `[profile.profiling]` Cargo profile (release codegen + line-table debug)
  exists specifically for this — `--release` selects it.

## Reading the output

- **Budget:** 60 FPS = 16.6 ms/frame; 100+ FPS = <10 ms. `profile.sh` runs with
  `--no-vsync` so the number is real work, not the swapchain wait.
- **Hot functions** are leaf self-time aggregated across all threads (Bevy runs
  its schedule across the `ComputeTaskPool`, so cost is spread). Entries tagged
  `[park/syscall in libc]` are idle/parked threads — discount them.
- **GPU vs CPU:** if disabling the heaviest GPU feature (e.g. shadows) doesn't
  move FPS and the main work isn't in `wgpu`/driver libs, you're CPU-bound.
- **`addr2line` line numbers are unreliable under thin-LTO** (inlining aliases
  them); trust the *function names and types*, not `file:line`.

## Anti-patterns this subsystem exists to catch

These are the recurring shapes of per-frame regressions. Prefer the
*by-design* fix (left) over remembering the rule (right):

| Make it impossible | …instead of relying on |
|--------------------|------------------------|
| Don't `impl Clone` for heavy, shared, read-only containers (e.g. a USD `TextReader`); share via `Arc` and borrow `&*arc`. Provide a loud `deep_copy()` only for the rare real need. | "remember not to write `(*arc).clone()`" |
| Do once-per-entity setup in an **observer** (`OnAdd<T>`) — the framework runs it exactly once. | a polling `run_if(Without<Marker>)` system that re-scans forever if any code path forgets to insert the marker |
| If you must poll, mark **every** examined entity (all `else { continue }` paths), or use a combinator that owns the insert. | hand-inserting the marker on only the success path |
| Gate per-frame systems on change (`Changed<T>`, `is_changed()`, a generation cursor) per `AGENTS.md §7`. | unconditional `Update` work for state that's stable most frames |

If a system that's *supposed* to be gated shows up in a steady-state profile at
all, its gate isn't closing — that's the bug, not the cost.

## Mechanics gotchas (encoded in the scripts; documented so they're not re-hit)

- samply `--save-only` profiles are **unsymbolicated** (raw addresses) — use
  `symbolicate_samply.py`, not a generic JSON reader.
- samply writes the capture **only when its child exits**; `--duration` just
  stops sampling. Our binaries ignore the API `Exit` (it rejects null params),
  so `profile.sh` SIGINTs samply's child PID to finalize. Never wrap samply in
  `timeout --signal` with a short `--kill-after` — it SIGKILLs samply mid-write.
- On Wayland the NVIDIA Vulkan swapchain is perpetually `VK_SUBOPTIMAL_KHR` (one
  WARN/frame, ~3 ms + latency); `WINIT_UNIX_BACKEND=x11` removes it for clean
  GPU-side comparisons.

## Roadmap (not yet built — greenlight to add)

- **Headless frame-budget regression test** on a GPU runner: load the default
  scene, assert steady-state `frame_time` under a budget. Would fail loudly on
  the PR that introduces a regression — the catch-all for classes no lint
  anticipates.
- **In-engine perf overlay plugin** (frame budget + worst-N system timings) for
  interactive spotting.
- A clippy/grep guard flagging `(*…).clone()` on `Arc<_>` in the USD crates.
