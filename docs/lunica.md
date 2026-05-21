# Lunica

**Lunica** is the Modelica-focused subset of LunCoSim. It is the
workbench formerly shipped as `modelica_workbench` — same code, new
name, narrower marketing surface.

Where the full LunCoSim client pulls in celestial mechanics, terrain,
mobility, robotics, and the cosim orchestrator, Lunica ships only the
Modelica modelling/simulation experience: code editor, schematic
diagram, package browser, simulator, and plots. That makes it small
enough to compile to a single desktop binary **and** to wasm32 for the
browser, with the same sources.

## What it is

- **Subset of LunCoSim**: same crates (`lunco-modelica`,
  `lunco-workbench`, `lunco-canvas`, `lunco-viz`, `lunco-doc`,
  `lunco-theme`, …), no Bevy renderer extras for celestial/terrain/etc.
- **Two binaries, one source tree**, both in `crates/lunco-modelica`:
  - `lunica` — desktop (native window via Bevy + bevy_egui).
  - `lunica_web` — wasm32, served via `scripts/build_web.sh`.
- **Rumoca** under the hood for parse/compile/sim. The desktop build
  reads MSL from `~/.cache/lunco/msl/` (pre-indexed); the web build
  bundles a pre-parsed MSL artifact at build time so the page doesn't
  pay a 27-min cold parse.

## What it isn't

- Not the full simulation client. No `big_space`, no Avian rover
  physics, no celestial ephemeris, no terrain.
- Not "the Modelica crate". That's `lunco-modelica` (library + bins).
  Lunica is the *application* assembled from it.

## Compiling

You need a working **Rust toolchain** (stable, current). The
workspace uses Bevy 0.18; first build is slow.

```sh
# Desktop
cargo run --bin lunica                  # opens the window
cargo run --bin lunica -- --api 3000    # also serves the typed-command HTTP API

# Web (wasm32)
./scripts/build_web.sh build lunica_web
./scripts/build_web.sh serve lunica_web   # http://localhost:8080
```

The web pipeline produces `dist/lunica_web/` (wasm + JS glue +
`index.html` + bundled MSL under `dist/lunica_web/msl/`).

## MSL cache & `msl_indexer`

Lunica needs the Modelica Standard Library on hand to type-check and
simulate. Currently this is a manual one-time bootstrap on the host
machine:

1. **Download / sync MSL sources** into the workspace cache under
   `~/.cache/lunco/msl/` (or wherever `lunco_assets::msl_source_root_path()`
   resolves to on your machine). Today this is done out-of-band — drop
   the Modelica Standard Library checkout there.
2. **Run the indexer** to produce rumoca's pre-parsed bincode cache:

   ```sh
   cargo run --release -p lunco-modelica --bin msl_indexer
   ```

   This populates the artifact cache rumoca consults on startup. Skip
   it and Lunica will re-parse MSL from source on first launch — minutes
   of wall time for what should be a cache hit.

The web build runs `lunco-assets`'s `build_msl_assets` over the same
on-disk MSL during `./scripts/build_web.sh build lunica_web`, packaging
a versioned compressed bundle into `dist/lunica_web/msl/`. That is the
artifact the wasm runtime fetches at page load — no host filesystem
access from the browser.

## Roadmap: self-bootstrapping

The current "download MSL yourself, then run `msl_indexer`" workflow
is a developer-only step. The intent is to move both inside Lunica:

- **In-app MSL download** with progress + integrity check, into the
  same cache layout the indexer expects.
- **Dynamic indexing** triggered by Lunica itself when the cache is
  missing or out of date — same code path as `msl_indexer` today,
  surfaced via the workbench rather than as a separate binary.

Until those land, treat the two-step bootstrap above as the supported
path.

## See also

- [`docs/architecture/11-workbench.md`](architecture/11-workbench.md) —
  workbench frame, perspectives, panel registration.
- [`docs/architecture/13-twin-and-workflow.md`](architecture/13-twin-and-workflow.md) —
  how Lunica fits alongside `sandbox` and `lunco_client`.
- [`docs/WEB_BUILD.md`](WEB_BUILD.md) — wasm pipeline reference.
- [`docs/api.md`](api.md) — typed-command HTTP API exposed by
  `lunica --api`.
