# Building LunCo Modelica Workbench for Web

## Prerequisites

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli
```

## Build

```bash
cargo build --release --target wasm32-unknown-unknown --bin modelica_workbench_web
wasm-bindgen target/wasm32-unknown-unknown/release/modelica_workbench_web.wasm \
    --out-dir crates/lunco-modelica/web/pkg --target web
```

## Run

```bash
cd crates/lunco-modelica/web
python3 -m http.server 8080
# Open http://localhost:8080/index.html
```

## Features

- **Bundled models**: Battery, BouncyBall, RC_Circuit, SpringMass (embedded at compile time)
- **Responsive canvas**: Automatically resizes with browser window
- **WebGPU rendering**: Via Bevy/wgpu (falls back to WebGL2)

## Notes

- Simulation requires Web Workers (not yet implemented)
- UI and rendering work fully in browser
- Chrome 113+ / Edge 113+ / Safari 16.4+ required
