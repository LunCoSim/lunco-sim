# Building LunCo Modelica Workbench for Web

## Prerequisites

```bash
# Install wasm32 target
rustup target add wasm32-unknown-unknown

# Install wasm-bindgen CLI
cargo install wasm-bindgen-cli
```

## Build

```bash
# Compile to WebAssembly
cargo build --release --target wasm32-unknown-unknown --bin modelica_workbench_web

# Generate JavaScript bindings
wasm-bindgen target/wasm32-unknown-unknown/release/modelica_workbench_web.wasm \
    --out-dir crates/lunco-modelica/web/pkg \
    --target web
```

## Run

```bash
# Serve locally
cd crates/lunco-modelica/web
python3 -m http.server 8080

# Open browser
# http://localhost:8080/index.html
```

## Requirements

- Chrome 113+ / Edge 113+ / Safari 16.4+
- WebGPU support (check `chrome://gpu`)

## Notes

- Simulation requires Web Workers (not yet implemented)
- UI and rendering work fully in browser
