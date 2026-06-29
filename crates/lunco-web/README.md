# lunco-web

Shared **web-frontend boot** library for LunCoSim wasm apps.

Two halves of one concern — dismissing the HTML loading screen the moment the
app is actually interactive (not a frame sooner):

- **Browser side** — `web/lunco-boot.{js,css}` (static assets, copied into every
  `dist/<app>/` by `scripts/build_web.sh`). `lunco-boot.js` exports
  `boot({ init, wasmUrl, wasmSize, title })`: it injects the loader card,
  streams the wasm download with a progress bar, forces
  `Content-Type: application/wasm` for streaming compile, calls the bundle's
  `init()`, and surfaces errors. A per-app `index.html` reduces to a ~15-line
  config call. `lunco-boot.css` carries the loader styles, the `#bevy`
  focus-ring fix, and the dark backdrop — themeable via `--lc-accent` /
  `--lc-backdrop` CSS variables.
- **Rust side** — **`WebReadyPlugin`** (this crate). After Bevy paints its first
  egui frame it calls `window.__lc_app_ready()`, which fades the loader out.
  Hiding earlier (on `init()` resolve) would flash a blank canvas during the
  plugin-build gap.

## Usage

```rust
app.add_plugins(lunco_web::WebReadyPlugin);
```

On native the plugin adds nothing (the `__lc_app_ready` call is a no-op), so the
same line compiles on every target.
