# Terrain LOD, shadows, and determinism — audit and fixes

Audit of the CDLOD surface streamer (`lunco-terrain-surface`, `lunco-terrain-core`) against
four reported symptoms, with every claim measured against the **real moonbase DEM**
(Shackleton connecting ridge, 3200² over 16 km ⇒ 5 m posting, 2482 m relief) rather than
argued from the code.

Scope: the **surface** streamer. `lunco-terrain-globe` (quad-sphere, orbital) is out of scope;
its handover to the surface path is unimplemented (`terrain-globe/src/lib.rs:20`).

---

## What was measured, and what it overturned

A first pass over this code produced a confident set of claims that **measurement then
falsified**. They are recorded here because the wrong version is intuitive and will be
re-derived by the next reader.

The diagnostic composed the real DEM with the twin's crater + overzoom stack and ran the
production selection walk (`crates/lunco-terrain-bake/tests/lod_budget_diagnostic.rs`).

**Claim 1 — "measured error never converges, so the tile count diverges." FALSE.**

The reasoning was that `node_error` gates the oracle at each node's *own* probe spacing
(`stream_viz.rs:1006-1015`), so a fractal oracle re-synthesises detail at every scale and the
error is scale-free. If error decayed slower than 2× per level, tiles per ring would grow and
the total would diverge. Against real data it decays at a geometric mean of **~2.1×**:

| depth | node side | error (m) | ratio |
|---|---|---|---|
| 0 | 8000 | 104.30 | — |
| 1 | 4000 | 32.50 | 3.21 |
| 2 | 2000 | 21.66 | 1.50 |
| 3 | 1000 | 6.89 | 3.14 |
| 4 | 500 | 3.44 | 2.00 |
| 5 | 250 | 2.02 | 1.71 |
| 6 | 125 | 0.62 | 3.26 |
| 7 | 62.5 | 0.32 | 1.91 |
| 8 | 31.25 | 0.28 | 1.14 |
| 9 | 15.6 | 0.083 | 3.42 |
| 10 | 7.8 | 0.049 | 1.69 |

~2.0 is exactly the CDLOD property (Strugar: LOD ranges must double, because each child is ¼
the area). The metric is healthy. The probe-step gating is *correct* — real DEM relief
dominates the coarse depths and it bottoms out because `Overzoom::min_radius = 0.4 m` is a
real floor.

**Claim 2 — "the budget fit exits over budget at the 32px clamp, so you get max-coarse AND
too many cells." FALSE on native, TRUE on wasm.**

| pixel_error | tiles | per-depth |
|---|---|---|
| 3.00 | **1015** | [0,0,0,11,151,180,182,231,260] |
| 4.80 | **508** | [0,0,1,23,119,84,97,104,80] |
| 7.68 | 262 | [0,0,3,33,61,44,54,31,36] |
| 12.30 | 139 | … |
| 19.70 | 109 | … |
| 31.40 | 91 | … |
| 32.00 | **88** | [0,0,12,12,12,12,12,12,16] |

Native (budget 512): 3 px → 1015 tiles → **one** coarsening rung to 4.8 px → 508. Fits. The
clamp is never approached.

Wasm (budget **64**): the fit walks every rung to the 32 px clamp and lands at **88 tiles,
still over budget**, with quality pinned at the floor. **The wasm budget is unreachable on
this terrain** — that is the real "really bad visual quality", and it is web-specific.

**Claim 3 — "`eye_height` floors the whole tree." Real arithmetic, negligible effect.**

`eye_height` is added in quadrature to *every* node (`quadtree.rs:466`), so it is a floor on
every node's distance and the tile under an avatar 30 m from the camera reads ~1.8× farther
than the tile under the camera. That much is true. But at DEM scale it barely moves the
selection: 1015 tiles at eye=0, 1015 at eye=20, 955 at eye=100. Left alone.

**Claim 4 — "the horizon map causes the darkness." FALSE, and it isn't a horizon map.**

`crates/lunco-environment/src/horizon.rs` is a **ray-march**; the angle-map design was tried
and rejected (`horizon.rs:6-10`). It cannot produce darkness: `horizon_march.wgsl:41` returns
**1.0 (lit)** when unbound, `horizon_shade.rs:159` clears `shadow_cache_on` with no cache, and
`terrain_geomorph.wgsl:444` gates the whole darkening block on that flag. Camera movement is
an input to neither bake. `reveal = 0` is likewise innocent — it is a *vertex* morph weight to
the parent lattice (`terrain_geomorph.wgsl:280-289`), fully opaque, never alpha.

---

## Findings and fixes

### 1. Terrain differs every launch — curvature body picked by archetype order ✅ FIXED

**The headline defect.** `sync_terrain_body_curvature` resolved the body via
`q_site.iter().next()` (`placement.rs:424`) — an arbitrary archetype-order pick — and that
body's radius folds into the `SurfaceOracle` as the **final** `BodyCurvature` modifier
(`oracle.rs:53`). So it decides the composed **geometry** and the `content_key` every
tile/derived cache keys on.

`TerrainGeoref` (`georef.rs`) carried lat/lon/height but **no body**, so there was nothing
authoritative to read — the global guess was structural.

A scene with a second `SiteAnchor` (ground stations author `lunco:anchor:body = 399` Earth)
could curve a lunar DEM to Earth's 6371 km radius, and *which* anchor won varied per launch
with async USD load order. Generation was a function of load order, not of the seed.

Fix — the body is now the terrain's own authored property:
- `TerrainGeoref::body: i32` + `DEFAULT_ANCHOR_BODY = 301`, matching the celestial bridge's
  own default (`usd-sim/celestial.rs:41`) so an unauthored scene cannot curve to one body and
  pin its site frame to another.
- The USD bridge reads `lunco:anchor:body` (`usd-terrain/lib.rs`).
- `sync_terrain_body_curvature` resolves the **body** by reducing over the terrains' own
  `TerrainGeoref`s (by authored id, not iteration order); terrains that disagree warn instead
  of letting load order decide.

**Scope kept deliberately narrow.** An intermediate version also sourced the globe-punch
*geodetic* from `TerrainGeoref` — which would have regressed any scene with a `SiteAnchor` but
no authored terrain georef (moonbase is exactly that), punching the globe at lat/lon 0,0
instead of the pole, because the georef default is 0,0. Only the body **radius** reaches the
oracle; *where* on the globe to punch remains the site anchor's job and is unchanged.

Verified non-vacuous by mutation: restoring the `q_site.iter().next()` pick fails 4 of the 5
tests, including the spawn-order one.

**Scenes should NOT need to author `lunco:anchor:body`, and moonbase should not.** It authors
no `lunco:anchor:*` at all and now defaults deterministically to Moon, which is correct —
authoring `body = 301` explicitly would change nothing, and its `center_lat/lon` are
write-only today (reprojection is deferred to offline tooling), so authoring those is inert
too. The attribute earns its keep only for a terrain that is **not** on the default body:
that is the case which previously had no way to be stated, which is exactly why the code fell
back to guessing from whatever `SiteAnchor` loaded first. The fix is "when unstated, default
deterministically", not "make every scene state it".

Unrelated observation while here: moonbase has no `SiteAnchor`, so it gets **no curvature** —
the DEM stays a flat tangent plane, ~4.6 m of sagitta at its ±4 km edges. That is only visible
if a globe is drawn beneath it (the seam curvature exists to close). If it is, the fix is a
`SiteAnchor`, not a `TerrainGeoref`. Note also that `mode_exposure` hardcodes
`MOON_RADIUS_M = 1.7374e6` (`ui/mod.rs:332`) and consults no anchor at all.

Pinned by `crates/lunco-celestial/tests/terrain_curvature_determinism.rs`: anchor spawn order
must not change curvature; a foreign anchor must not hijack; the terrain's authored body wins;
unauthored defaults to Moon; no anchor ⇒ no curvature.

Note this is the **second** determinism bug of the same shape. The first (USD `children()`
hash-order randomising the layer-stack fold) was fixed by canonical sorts, and both survive
(`usd-terrain/lib.rs:197-198`, `edits.rs:143-145`). The pattern to watch: **anything that
reaches the oracle must be a pure function of the document.**

Ruled out while hunting: crater/rock scatter (`ChaCha8Rng` via `rng_for(seed, salt)`,
`sampler.rs:74`), overzoom (pure lattice hashes), and both canonical sorts.

### 2. Budget fit froze coarse on a parked camera ✅ FIXED

`last_fit_px` is a one-way ratchet that recovers **one rung per selection** — but the idle
signature (`stream_viz.rs:928-952`) hashed focus, eye, gen, oracle pointer and the three LOD
knobs, and **not `last_fit_px`**. Trace: sweep dense terrain → coarsen → camera stops → the
pass that stopped it climbs one rung → next frame the signature matches → body skipped → the
remaining rungs never run. A parked camera stayed coarse for the session.

Fix: `last_fit_px` joins the signature. While the fit is coarser than `base_px` the signature
keeps changing, so the climb runs to completion; once it reaches `base_px` the value stops
changing and the gate goes quiet on its own.

### 3. Budget silently unreachable ✅ FIXED

The coarsen loop exits at the clamp regardless of `sel.len()`, which on wasm means drawing
88 tiles against a 64 budget with quality pinned at the floor — indistinguishable from "this
terrain is just ugly". Now latched + warned once per episode (`LodTiles::budget_unreachable`),
naming the real knobs (`tile_budget` / `max_depth`). `32.0` is now `FIT_PX_MAX` with the
measured rationale attached.

**Still open (a decision, not a bug):** the wasm budget of 64 is unreachable on moonbase. The
measured floor is 88 tiles. Either raise the wasm budget to ~128 or lower `max_depth` for web.
Left to you — it is a real perf/quality tradeoff, not something to guess.

### 4. `pulls_terrain_detail` — one predicate, was four ✅ FIXED

The audit's original recommendation here was **wrong** and worth recording. It claimed the
`RigidBody::Dynamic` filter starves proxies/on-foot avatars and should widen to `Kinematic`.
But `collider_ring.rs:245` and `:493` filter `Dynamic` **identically** — and that agreement is
the entire point of `refine_selection_at`: the visual cover is forced to max depth around
exactly the bodies the ring bakes colliders for. Widening only the visual filter would
*create* the mismatch it claims to fix (refined ground under a proxy with no ring).

The real defect: this load-bearing predicate was spelled out four times independently. Now one
`pulls_terrain_detail()` (`stream_viz.rs`) with the invariant documented, called from all four
sites. Semantics unchanged — deliberately.

### 5. Exposure — silent frame fallback ✅ FIXED

`mode_exposure` (`sandbox/src/ui/mod.rs:312-315`) resolved the camera position via
`cell` + a **direct** `ChildOf` → `Grid` edge, and silently fell back to `tf.translation`
otherwise. Possess a vessel and the camera reparents to it, losing both — so a metres-scale
offset from the vessel was substituted for a grid-absolute geographic position, `local_up`
collapsed to +Y, and sun elevation jumped to the site origin's value. At a polar site
(moonbase is lat −89.46°) elevation sits permanently *inside* the 0.02 rad ramp band, so that
jump lands mid-ramp and moves EV.

Fix: no fallback. A wrong frame is a different question, not a degraded answer — hold the last
exposure and `warn_once`. Same family as the `camera_mount` bug: a parent-identity assumption
that degrades silently.

**Honest calibration:** an earlier draft claimed this swings 6 stops. It does not. Moving 4 km
across the site tilts `local_up` by only ~0.13° ≈ 11% of the ramp band ⇒ **~0.7 stops** —
visible over the 1.5 s ease, but the 6-stop swing needs an orbital transition (`ui/mod.rs:306`
hard-switches `target` with no ramp). The user-reported "goes dark" is most likely this
*plus* finding 6.

### 6. Black holes on fast motion — OPEN (the real "goes dark")

Not fixed here; it is the largest change and wants your call on approach.

Tiles bake at `bakes_per_frame` = 4 native / **1 wasm** (`stream_viz.rs:412-425`), so a fresh
512-tile selection with a cold cache takes **≥128 frames (~2.1 s)** native. And there is **no
coarse stand-in at all**: `CARPET_DEPTH = 2` is only a *sort key* over nodes already in `sel`,
and the selection is a REPLACE cover. Measured on moonbase, the per-depth selection at 3 px is

```
depth  0   1   2    3    4    5    6    7    8
tiles  0   0   0   11  151  180  182  231  260
```

— **depths 0–2 are empty, so the carpet branch never fires.** The comment claiming it covers
the view in a low-res carpet describes a mechanism that selects nothing on this terrain.

The despawn `retain` (`:1318-1336`) keeps a stale tile only where it overlaps a wanted-but-not-
fresh node — protection against pulling a tile out from under a hole, **not** coverage. Pan or
teleport somewhere new and nothing overlaps ⇒ nothing renders ⇒ clear colour.

The industry answer is unambiguous and it is *not* prefetch (MSFS explicitly denies
pre-downloading; the only velocity prefetcher found is Space Engineers' 0.33 s lead, and it is
physics-only). It is **render the parent while the child bakes**:

- MSFS: *"Draw tiles using the best currently available data ● Tiles can use data from a
  parent (e.g. aerial texture)"* — one line, no stall, blur instead of nothing.
- Cesium: `ForbidHoles` — *"unrefine back to a parent tile when a child isn't done loading…
  never rendered with holes, though the tile rendered instead may have low resolution"*;
  `skipLevelOfDetail` renders children alongside parents.
- Geometry clipmaps: coarse levels are **always resident** — Losasso & Hoppe deactivate only
  the *finest* levels, and not for memory: dense fine levels at altitude *cause aliasing*.

⚠ **Do NOT "keep the coarse pyramid resident and draw it underneath".** That is the obvious
design and it is wrong for a heightfield: a depth-0 node carries **104 m** of measured error
(table above), so its surface does not sit *below* the fine surface — it *crosses* it, and
would punch through as a blocky shell. Cesium does not underlay; `ForbidHoles` **unrefines** —
it draws the parent *instead of* its children, so the cover stays exact and disjoint.

Concretely, two parts:
1. **Always bake + retain depth ≤ `CARPET_DEPTH` nodes**, selected or not — on moonbase that
   is 1+4+16 = **21 tiles**, trivial, and it guarantees a fallback surface exists over the
   whole footprint from the first frames. Never evict them in the cache trim (`:1355`
   currently drops all non-resident entries in one shot past `CACHE_CAP`).
2. **At spawn, substitute the nearest already-baked ANCESTOR for any selected node that is not
   yet baked**, replacing that whole subtree in the cover (dedup — many children collapse to
   one ancestor). As each fine node lands it replaces the ancestor. This is unrefinement, not
   underlay: exactly one tile covers any point at any instant, so no z-fighting and no holes.

Also sort near-field before far-field.

⚠ Cesium's `wasCreatedByUpsampling()` is the flag to steal alongside it: **blur the picture,
never the ground.** A collider must never be cooked from interpolated parent data.

### 7. Shadows — architecture is right; the acceleration is missing

**Keep the design. Do not bake.** Tiles are `NotShadowCaster` (`stream_viz.rs:684`) so CSM
carries only *object* shadows onto terrain, while terrain-on-terrain self-shadowing comes from
the ray-march. That split is what UE5, Fortnite pre-Ch4, and Timonen 2013 §9 all converge on,
and `horizon.rs`'s rejection of an angle map is vindicated: Timonen & Westerholm report sharp
edges on planar content need **up to 128 azimuths**; 8-bit angle quantisation at d = 30 km is a
**210 m** terminator error; 2048²×64 azimuths is **256 MB**; and the line-sweep needs scattered
writes, so it **cannot run on WebGL2** at all.

Cascades cannot be rescued at grazing sun, structurally: required bias scales as `1/tan(e)` (at
3°, a 1 m bias detaches the shadow by **19 m**), texel footprint stretches by `1/sin(e)` =
**19×**, the caster is 30 km from the receiver, and AMD's standard terrain-shadow optimisation
(back-face culling against the shadow camera) *inverts* at grazing sun. Epic's Fortnite Ch4
postmortem is the cautionary tale: they tried caching a moving sun's pages, got *"too many
artifacts at page boundaries"*, and gave up — Landscape was their single biggest VSM offender.

**Highest-value change: max-mipmap the existing ray-march** ([arXiv 2005.06671] — a *lunar
polar landing simulator*, i.e. this exact problem: airless body, DEM heightfield, sun near the
horizon, umbrae >30 km). **5.4 ms vs 18.2 ms** uniform stepping (237%), **O(log N)**, pure
fragment shader + a mip chain (WebGL2-safe), rebuilt only on significant frustum change.
Their low-end datapoint: **>50 Hz at 1080p on an Intel Iris 540**.

On baking: **nobody in the mission-sim world bakes terrain shadows** — not the lunar paper,
not Airbus SurRender (raytraced, 5 Hz), not JPL, not Cesium. They must be correct at arbitrary
epochs. But the sun rate here *is* an advantage: **0.55°/hour** vs 15°+/hour for every engine
fighting this. Anything cached stays valid for hours of sim time, which the existing
`sun_threshold_deg: 0.05` re-bake gate already exploits. If baked masks are ever wanted:
rebake async, **never interpolate slices** — cross-fading a binary terminator ghosts.

Where a horizon map *does* belong is **ambient**: 8–16 azimuths, 8-bit, ~32 MB at 2048²,
bakeable in a worker, incrementally per-tile (ambient horizon is local — the *direct-sun*
horizon needs a 1000-texel apron at 30 m/texel and 30 km shadows, larger than the tile).
Timonen's Eq. 5 gives the closed form; weight azimuths by rim sunlit-ness and you reproduce
the broad-arc secondary illumination the Artemis PSJ paper measures (**>90°** variation in
lighting direction inside small craters — the fill arrives from an arc of sunlit rim, not a
point).

The existing penumbra term `(ray_height - terrain_height) / (distance * tan(sun_radius))` is
**correct** and validates against the lunar paper's published numbers (d = 30 km, e = 3° ⇒
5.3 km penumbra ≈ 1/5.7 of shadow length; paper says *">5 km or nearly 1/7th"*). One free
refinement: the occluded fraction of a uniform disc is the circular-segment area, not linear —
`f(x) = (1/π)(acos(x) − x√(1−x²))`, `x = Δh/w⊥`. `smoothstep` approximates it; linear is
visibly wrong at the edges. Also worth calibrating: measured PSR illumination is **2–3 orders
of magnitude** below sunlit terrain ⇒ 0.1–1% fill; `SHADOW_FILL = 0.26` and
`HORIZON_FILL_FLOOR = 0.22` are not in physical units.

---

## Dead code surfaced (not yet removed)

- `HorizonSpec::azimuths` — declared (`markers.rs:129`), parsed from USD
  (`usd-bevy/src/lib.rs:809`), **never read**. Vestige of the rejected angle-map design; its
  doc comment (`markers.rs:105-115`) still describes a system that does not exist.
- `HorizonSpec` default resolution is **512** (`markers.rs:134`); both twins author 2048.
- `select_3d`, `refine_range`, `geometric_error`, `select_with_error_budgeted` — zero
  non-test callers. `root_geometric_error` is consequently a dead field on the live path
  (`select_with_error` uses `range_factor · node_error`). The module doc at `stream_viz.rs:8`
  claiming `select_3d` is used is stale.
- `stream_viz.rs:1280` `TODO(R1)`: **the wasm tile cache always misses** — every tile re-bakes
  every session, against a 64-tile budget at 1 bake/frame.

---

## Primary sources

- Strugar, F. (2010). *Continuous Distance-Dependent Level of Detail for Rendering Heightmaps.*
  <https://aggrobird.com/files/cdlod_latest.pdf> · <https://github.com/fstrugar/CDLOD>
- Losasso & Hoppe (2004). *Geometry Clipmaps.* SIGGRAPH. <https://hhoppe.com/proj/geomclipmap/>
- Timonen & Westerholm (2010). *Scalable Height Field Self-Shadowing.* CGF 29(2).
  <http://wili.cc/research/hfshadow/hfshadow.pdf>
- Timonen (2013). *Screen-Space Far-Field Ambient Obscurance.* HPG. <http://wili.cc/research/ffao/ffao.pdf>
- *Optimally Fast Soft Shadows on Curved Terrain with Dynamic Programming and Maximum Mipmaps.*
  <https://arxiv.org/pdf/2005.06671> — **lunar polar landing sim; closest analogue to this project**
- Fuentes, L. / Asobo (2022). *Designing the Terrain System of Microsoft Flight Simulator.* GDC.
  <https://www.asobostudio.com/files/inline-images/Designing_Terrain_System_Fuentes_Lionel.pdf>
- Lauritzen & Olsson / Epic (2023). *Virtual Shadow Maps in Fortnite Chapter 4.*
  <https://www.unrealengine.com/tech-blog/virtual-shadow-maps-in-fortnite-battle-royale-chapter-4>
- Chajdas / AMD (2016). *Optimizing Terrain Shadows.* <https://gpuopen.com/learn/optimizing-terrain-shadows/>
- Cesium 3D Tiles spec + `Cesium3DTileset` reference. <https://github.com/CesiumGS/3d-tiles>
- KSP2 Dev Insights #10 (Collisions) & #12 (Planet Tech).
  <https://forum.kerbalspaceprogram.com/topic/205930-developer-insights-12-planet-tech/>
- *Dynamic Secondary Illumination in Permanent Shadows within Artemis III Candidate Landing
  Regions.* PSJ. <https://iopscience.iop.org/article/10.3847/PSJ/ad1b50>
