# lunco-web

Shared **web-frontend boot and tool-host** library for LunCoSim wasm apps.

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
- **HTML/CSS/Rhai tools** — `mountRhaiTool({ htmlUrl, cssUrl, root })` mounts a
  trusted HTML fragment and binds its explicit `data-rhai` actions or
  `<script type="application/rhai" data-target="...">` blocks to the existing
  `lunco_rhai` wasm bridge. The page remains presentation; command authority
  stays in the typed engine command bus. The generated page exposes this as
  `window.lunco_mount_rhai_tool` after importing the boot module.
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

## HTML + CSS + Rhai

The browser host can mount a tool bundle after the wasm app is ready:

```js
const tool = await window.lunco_mount_rhai_tool({
    htmlUrl: './tools/rover.html',
    cssUrl: './tools/rover.css',
    root: '#tool-dock',
});
```

The HTML fragment may use a short action attribute:

```html
<button data-rhai="pause()">Pause</button>
<output data-rhai-status></output>
```

For multi-line behavior, use an explicit Rhai script block:

```html
<button id="reset">Reset</button>
<script type="application/rhai" data-target="#reset" data-event="click">
restart_scene()
</script>
```

`mountRhaiTool` returns `dispose()` so a tool can be replaced without leaving
event listeners or styles behind. This surface is intentionally browser-only
because the native editor currently renders with egui; native HTML requires a
separate embedded-webview surface and is not silently emulated.
