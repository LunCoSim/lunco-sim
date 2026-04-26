# Building LunCo Modelica Workbench for Web

## Overview

The LunCo Modelica Workbench compiles to WebAssembly (wasm32-unknown-unknown) and runs in the browser with WebGPU rendering via Bevy/wgpu. The full Modelica simulation pipeline works in the browser — compilation, DAE generation, and runtime simulation.

## Architecture

### How it works

```
┌─────────────────────────────────────────────────────────────┐
│                    Browser (WebAssembly)                     │
├─────────────────────────────────────────────────────────────┤
│  .mo files → rumoca-session → DAE → SimStepper → outputs    │
│       (rumoca-sim with wasm32 time support)                  │
│                                                              │
│  UI: Bevy + Egui (WebGPU/wasm-bindgen)                      │
└─────────────────────────────────────────────────────────────┘
```

### Key difference from desktop

On desktop, `ModelicaPlugin` spawns a background thread (`thread::spawn`) that owns `SimStepper` instances. On wasm32, threads don't exist, so an `InlineWorker` resource processes commands synchronously in a Bevy system.

### The wasm32 time problem

`std::time::Instant` **panics** on `wasm32-unknown-unknown` because browsers restrict high-resolution monotonic clocks (Spectre mitigation). Rumoca's upstream `rumoca-sim` uses `std::time::Instant` directly in 3 places.

**Fix**: A local fork (`external/rumoca/`) on the `web-fix` branch replaces those imports with conditional compilation:

```rust
#[cfg(target_arch = "wasm32")]
use instant::Instant;      // → performance.now() via wasm-bindgen
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
```

See `external/rumoca/` for the fork. Maintained on `web-fix` branch at `LunCoSim/rumoca`.

## Prerequisites

```bash
# Required: wasm32 target
rustup target add wasm32-unknown-unknown

# Required: wasm-bindgen CLI (the build script also looks at
# .cargo-bin/bin/wasm-bindgen if you keep a project-local copy).
cargo install wasm-bindgen-cli

# Strongly recommended: wasm-opt (binaryen). Shrinks the release wasm
# by ~30 % and cuts in-browser compile time proportionally — a
# one-time install with no code changes. The build script auto-detects
# it on PATH and runs `wasm-opt -O2 --strip-debug` after wasm-bindgen.
sudo apt install binaryen          # Debian/Ubuntu (preferred)
# or:
cargo install --locked wasm-opt    # works too, but compiles binaryen from C++
# or:                              # download a prebuilt release:
# curl -L https://github.com/WebAssembly/binaryen/releases/latest/download/...

# Optional: Node.js http-server (recommended, fallback to python3)
npm install -g http-server
```

Verify wasm-opt is wired in:

```bash
which wasm-opt && wasm-opt --version
./scripts/build_web.sh build modelica_workbench_web 2>&1 | grep wasm-opt
# Expect:  [INFO] wasm-opt: 103.9 MB → 69.7 MB
```

If wasm-opt isn't on PATH the build still succeeds — the script logs a
hint and skips the optimisation pass.

## Quick Build (Recommended)

A convenience script is provided for Linux/macOS:

```bash
# Build WASM and generate bindings
./scripts/build_web.sh build

# Serve locally
./scripts/build_web.sh serve

# Or build and serve in one command
./scripts/build_web.sh all
```

## Manual Build

```bash
# Compile to WebAssembly
cargo build --release --target wasm32-unknown-unknown --bin modelica_workbench_web

# Generate JavaScript bindings
wasm-bindgen target/wasm32-unknown-unknown/release/modelica_workbench_web.wasm \
    --out-dir crates/lunco-modelica/web/pkg --target web
```

### Build times (approximate)
| Profile | Time | Output size |
|---------|------|-------------|
| dev | ~2 min | ~1.3 GB (too large for web) |
| release | ~3 min | ~73 MB WASM + 122 KB JS |

## Run

```bash
# Option 1: Using the build script
./scripts/build_web.sh serve

# Option 2: Using http-server (recommended)
cd crates/lunco-modelica/web
http-server -p 8080 -c-1 --cors

# Option 3: Using Python (fallback)
cd crates/lunco-modelica/web
python3 -m http.server 8080

# Open in browser
# http://localhost:8080/index.html
```

## Requirements

- **Chrome 113+** / **Edge 113+** / **Safari 16.4+**
- WebGPU support (check `chrome://gpu`)
- HTTP server (file:// won't work for WASM)

## Features

| Feature | Status | Notes |
|---------|--------|-------|
| UI rendering | ✅ | WebGPU via Bevy/wgpu (falls back to WebGL2) |
| Canvas resize | ✅ | Auto-resizes with browser window |
| Model compilation | ✅ | rumoca-session compiles .mo → DAE |
| Simulation | ✅ | SimStepper runs in inline worker |
| Graph plotting | ✅ | egui_plot for time-series |
| Parameter tuning | ✅ | Recompiles with new values |
| Model inputs | ✅ | Runtime injection without recompile |
| Bundled models | ✅ | Battery, BouncyBall, RC_Circuit, SpringMass |

## File Structure

```
scripts/
└── build_web.sh              # Automated build script (Linux/macOS)
crates/lunco-modelica/
├── web/
│   ├── index.html          # Minimal HTML with canvas + JS loader
│   └── pkg/                # Generated by wasm-bindgen
│       ├── modelica_workbench_web_bg.wasm   # WASM binary
│       ├── modelica_workbench_web.js        # JS glue
│       └── *.d.ts          # TypeScript declarations
├── src/
│   ├── bin/
│   │   ├── modelica_workbench.rs      # Desktop binary
│   │   └── modelica_workbench_web.rs  # Web entry point
│   ├── models.rs                      # Bundled .mo files (include_str!)
│   └── lib.rs                         # Core + inline worker for wasm32
└── Cargo.toml
```

## Performance — Time-to-Interactive

The release wasm is large (104 MB pre-opt) and the page boots through
several stages. Three levers are wired into the build + page; install
them once and the rest is automatic.

### 1. `wasm-opt` (build step, ~30 % smaller wasm)

`scripts/build_web.sh` runs `wasm-opt -O2 --strip-debug` after
`wasm-bindgen` if the binary is on PATH. Typical impact:

```
wasm-opt: 103.9 MB → 69.7 MB
```

Smaller wasm = less to download, less for the browser to compile.

### 2. Streaming compile (page-side, free)

`crates/lunco-modelica/web/index.html` fetches the wasm with a
`TransformStream` for progress accounting and hands the live `Response`
to `wasm-bindgen`'s `init()`. The browser pipes that into
`WebAssembly.instantiateStreaming`, compiling chunks **as they
download** instead of buffering the whole 70 MB first. Roughly halves
the gap between "click" and "first frame".

### 3. `binaryen` post-pass + brotli/gzip on the wire (optional)

`python -m http.server` doesn't compress. If you serve from a server
that supports `Content-Encoding: br` (e.g. `caddy file-server`,
`miniserve --http=...`, or any production CDN) the network leg drops
another ~3×. Pre-compressing once at build time also works — drop a
`.wasm.br` next to the `.wasm` and serve with the right MIME headers.

### What still costs time

- **Bevy plugin construction** at App boot. Auditing
  `[workspace.dependencies] bevy = { features = [...] }` to drop
  `bevy_audio`, `bevy_pbr`, `bevy_gltf`, `bevy_sprite_render`,
  `bevy_ui_render`, `bevy_text`, `bevy_animation`, `bevy_scene`
  (none used by Modelica workbench) would cut another big chunk —
  this hasn't been done yet because it's shared with the rover/viz
  binaries.
- **MSL bundle fetch** (~16 MB compressed: 2 MB sources + 14 MB
  pre-parsed AST). Non-blocking — the workbench is fully usable
  while it streams in. Status shows in the bottom egui status bar;
  click for history.

## Output Layout

```
dist/<binary>/
  modelica_workbench_web.js          # wasm-bindgen JS glue
  modelica_workbench_web_bg.wasm     # post-wasm-opt binary
  modelica_workbench_web.d.ts        # TypeScript declarations
  index.html                         # copy of crates/.../web/index.html
  msl/
    manifest.json                    # bundle metadata + content hashes
    sources-<sha>.tar.zst            # ~2 MB MSL source bundle
    parsed-<sha>.bin.zst             # ~14 MB pre-parsed StoredDefinitions
target/wasm32-unknown-unknown/release/<binary>.wasm   # cargo's raw output
target/web/<binary>/                 # wasm-bindgen intermediate
.cargo-bin/                          # optional local wasm-bindgen install
```

Source `index.html` lives at `crates/<crate>/web/index.html` (committed
template); the build script copies it into `dist/` on every build. The
`dist/` and `.cargo-bin/` directories are git-ignored.

## Maintaining the Rumoca Fork

The fork lives at `LunCoSim/rumoca`. The web build pulls in branch
`wasm-asset-loader`, which adds `Session::load_source_root_in_memory`
on top of the existing `main` (which already carries the `Instant` /
`thread::spawn` wasm fixes).

Local development typically uses a sibling worktree at
`../rumoca/` and `path = ...` deps in `lunco-modelica/Cargo.toml` /
`lunco-assets/Cargo.toml`. To update:

```bash
cd ../rumoca
git fetch origin
git checkout wasm-asset-loader
git rebase origin/main    # replay our diff on top of upstream
git push --force-with-lease
```

Once published, the workspace can flip back from `path = ...` to
`git = "https://github.com/LunCoSim/rumoca", branch = "wasm-asset-loader"`.

To verify the fork is wired in:

```bash
cargo metadata --format-version 1 | jq '.packages[] | select(.name == "rumoca-sim") | .source'
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `time not implemented on this platform` | A direct `std::time::Instant` usage | Replace with `web_time::Instant` |
| `thread::spawn` / `failed to spawn thread` | Raw `std::thread::spawn` runs on wasm | Use `bevy::tasks::AsyncComputeTaskPool::get().spawn(async {…}).detach()` |
| Blank page / dark canvas, no UI | Wasm loaded but Bevy hasn't painted yet | Check console for plugin-build panics; the centred loader hides on first egui frame |
| 404 on `modelica_workbench_web.js` | Stale `dist/` after layout change | Re-run `./scripts/build_web.sh build …` to regenerate |
| `[MSL] failed: …` in status bar | `dist/<bin>/msl/manifest.json` missing or corrupt | Re-run build; the `build_msl_assets` step regenerates it |
| Compile finishes but model errors `unresolved type reference: Modelica.*` | First compile fired before MSL bundle was ready | Wait for status bar to show "MSL · ready" then click Compile again |
| `wasm-opt` step says `not installed` | binaryen not on PATH | See Prerequisites; install or skip |
