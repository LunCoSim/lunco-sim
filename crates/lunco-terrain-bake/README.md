# lunco-terrain-bake

The pure **DEM bake pipeline** — the same code both native and the browser run.

Factored out of `lunco-terrain-surface` so it depends on **no bevy and no avian**
(only `lunco-terrain-core`, `lunco-obstacle-field`, `tiff`, `serde`, `bincode`).
That leanness is the point: a Web Worker can run the whole bake without linking a
Bevy `App`.

## Why

On wasm32 there are no OS threads, so Bevy's `AsyncComputeTaskPool` degrades to the
page's **main thread** — and the moonbase DEM bake (a ~40 MB GeoTIFF decode plus
thousands of additive crater stamps) froze the tab for 15-30 s. This crate moves
that compute into a real Web Worker while keeping native on its threaded path, with
**one** implementation shared verbatim.

## What it does

`decode_raw` (the expensive GeoTIFF decode) → `finish_bake` (crop → resample →
intelligent upscale → apply the serializable `StampSpec`s) → a `HeightGrid`.
`bake_grid` is the single-pass native convenience (`decode_raw` + `finish_bake`).

| Item | Role |
|------|------|
| `dem` | GeoTIFF decode + `metadata.yaml` parse → `HeightGrid` (moved from terrain-surface). |
| `bake` | `crop_centered` / `resample` (pure, `HeightSource`-based). |
| `stamp` | crater placement + stamp (`stamp_spec_craters`, deterministic from a seed). |
| `DemBakeJob` / `StampSpec` | serializable bake inputs — so the worker reconstructs the SAME stamps without holding a `dyn TerrainLayer`. |
| `worker_client` (wasm) | main-thread client over `lunco-worker-transport`: transfers the tif, drains coarse+full replies. |
| `bin/dem_worker` (wasm) | the companion Worker binary: `decode_raw` once, then emit `BakeStage::Coarse` then `BakeStage::Full`. |

## Progressive

The worker decodes once, then emits a **coarse** preview grid (`COARSE_RES`) so the
terrain + collider ring appear and rovers settle, then the **full** grid — which
`lunco-terrain-surface` swaps in through its live re-stamp path (tiles refine
near-camera-first, no despawn flash).

Only the avian `Collider::heightfield` + Bevy `Mesh` derive stays in
`lunco-terrain-surface`, where those types live. See
[`docs/architecture/terrain-substrate.md`](../../docs/architecture/terrain-substrate.md).
