# Bevy 0.18 → 0.19 Migration Analysis

Status: **not started**. Bevy 0.19 shipped 2026-06-19. Workspace on 0.18.1.
Analysis date: 2026-07-03.

## TL;DR

- The two headline 0.19 overhauls (**render-graph-as-systems**, **parley text**)
  barely touch us: **no custom `ViewNode`/`RenderGraph` in the workspace**, and
  bevy_text is used in exactly **one** real site (rest is egui).
- Real work is a **dependency bump fan-out** + a handful of mechanical sweeps
  (`rand` 0.10, `TextFont`, `#[reflect(Resource)]`, `Assets::get_mut`).
- **2 hard blockers**: `transform-gizmo-bevy` and `avian_pickup` have **no
  0.19-compatible release** (last publish predates 0.19). Migration is gated on
  those upstreams (or forks/vendoring).

## Dependency bump matrix

Verified against crates.io + upstream GitHub PRs on 2026-07-03.

### Ready — 0.19 release published
| Crate | Current (0.18) | Target (0.19) | Published |
|---|---|---|---|
| `bevy` (+ all `bevy_*` sub-pins) | 0.18.1 | **0.19** | 2026-06-19 |
| `avian3d` | 0.6.1 | **0.7** | ✓ |
| `bevy_egui` | 0.39.1 | **0.40.1** | ✓ |
| `bevy-inspector-egui` | 0.36.0 | **0.37.0** | ✓ |
| `bevy_replicon` | 0.40.3 | **0.41.1** | 06-24 |
| `lightyear` | 0.27 | **0.28.0** | 06-26 |
| `leafwing-input-manager` | 0.20.0 | **0.21.0** | 06-22 |
| `bevy_enhanced_input` | 0.24.4 | **0.26.0** | 06-19 (repo moved `projectharmonia`→`simgine`) |
| `bevy_transform_interpolation` | 0.4.0 | **0.5.0** | 06-20 (may be transitive via avian 0.7) |

Also bump the lock-step sub-crate pins in root `Cargo.toml`:
`bevy_reflect`, `bevy_mesh`, `bevy_shader`, `bevy_asset`, `bevy_picking` → 0.19.

### BLOCKERS — no 0.19 release on crates.io
| Crate | Current | 0.19 PR | Path |
|---|---|---|---|
| **`big_space`** | 0.12.0 (pinned in member crates, not root) | **PR [#73](https://github.com/aevyrie/big_space/pull/73) "update to bevy 0.19"** — open, fresh (06-22, updated 06-30), "only minor fixes for tests/examples". +PR #76 name-cleanup. | git-dep on branch until merged/released |
| **`avian_pickup`** | 0.5.0-rc.1 | **PR [#33](https://github.com/janhohenheim/avian_pickup/pull/33) "Bevy 0.19"** — open, fresh, tested on examples+author's game. Bumps pickup 0.5→0.6, bevy 0.18→0.19, avian 0.6→0.7, rand 0.9→0.10 | git-dep on branch |
| **`transform-gizmo-bevy`** | 0.9.0 (last publish 2026-03-28) | **NONE** — no 0.19 bump PR open (top PR #108 is an unrelated reverse-Z ortho fix). Worst-off. | **fork + bump ourselves** |

`big_space` is the highest-stakes blocker — floating origin is core to the space
sim (used in `lunco-sandbox` `WorldShellPlugin`/`FloatingOrigin`, `lunco-networking`
`CellCoord` sync, USD tests). Its 0.19 branch exists and is described as a small
diff, so the risk is low but the merge/release is not in our control.

### Blocker options
1. **git-dep on the open PR branches** for `big_space` (#73) and `avian_pickup`
   (#33) — both are fresh and claim clean 0.19 ports. Fastest unblock.
2. **Fork + bump `transform-gizmo-bevy` ourselves** — no one upstream has started
   it. It's a thin wrapper over `transform-gizmo` core; only used for the editor
   gizmo (`gizmo_picking_backend`).
3. **Feature-gate all three OFF** to land the bevy/avian/egui/lightyear bumps
   first, then re-enable as branches merge. Gizmo + pickup are editor/interaction
   niceties; big_space gating is heavier (touches sandbox world root + net sync),
   so prefer the git-dep for it.

## Breaking changes — impact ranked (grounded in this codebase)

### 1. Cargo feature collections moved — LOW, do first
`bevy_window`, `bevy_input_focus`, `custom_cursor` dropped from `default_app`.
Root `Cargo.toml` explicitly lists `bevy_input_focus` already, so add the moved
features to the workspace `bevy` feature list. Net positive: the `--no-ui`
server binary compiles even fewer deps. Also: `system_font_discovery` is now an
opt-in feature (needs `libfontconfig1-dev` on Linux) — only matters if we do
system-font fallback (we don't; egui owns UI fonts).

### 2. `rand` 0.10 / `glam` / `uuid` bumps — LOW/MED, mechanical
21 direct `rand` sites. `RngCore` trait → `Rng`, old `Rng` → `RngExt`. Fix
imports. (Obstacle-field generator, terrain, etc.) See rand 0.10 book.

### 3. `#[reflect(Resource)]` semantics — LOW/MED
15 sites. `ReflectResource` is now a ZST; `#[reflect(Resource)]` also reflects
`Component`. Only breaks code that *pulls* `ReflectResource` to mutate (BRP,
world-serialization). Most sites are transparent. Cross-check with the known
`reflect_auto_register` link-overflow note — keep explicit `register_type`.

### 4. `Assets::get_mut` → `AssetMut` — MED, aligns with caching work
Returns `AssetMut<A>` (needs `mut` binding); fires `AssetEvent::Modified` **only
on real mutation**. Per-frame material-animation sampling must bind `mut` and
guard writes (`if field != new { field = new }`). This is *exactly* the
change-gated-derivation pattern already being rolled out (Substrate A / the
per-frame material sampler) — migration and the efficiency work converge here.

### 5. `TextFont` field type changes — LOW
Only real bevy_text site: `crates/lunco-celestial/src/missions.rs:200`
(`Text2d` + `TextFont { font_size: 100.0 }`). Wrap `FontSize::Px(100.0)`; if a
`font` handle is set, add `.into()` (now `FontSource`). All other `font_size`
hits are egui, untouched.

### 6. `bevy_scene` → `bevy_world_serialization` rename — MED, verify
The `DynamicScene`/BSN serialization crate was renamed. Runtime `Scene` +
`SceneRoot` (used for glTF composition in `lunco-usd-bevy`, ~6 sites) should
stay, but the **feature flag name** and any `use bevy::scene::…` paths need
checking. We spawn `SceneRoot(scene_h)` for glTF payloads — confirm the import
path survives the split.

### 7. Custom render phases / `DirtySpecializations` — N/A
Only relevant to custom render phases. We have **none** (no `ViewNode`,
`RenderGraph`, `add_render_graph_*`). Skip.

### 8. Custom materials (`AsBindGroup`) — LOW, verify
2 sites: `lunco-materials/src/shader_material.rs` (canonical `ShaderMaterial`)
and `lunco-celestial/src/trajectories.rs`. `AsBindGroup` is stable across 0.19;
verify derive + `Material` impl compile. Recheck the `shaderPath→library`
storm/blink fix survives.

### 9. Bloom now linear-space luma — LOW, visual
Bloom looks subtly different. Re-check against the lighting/exposure pass;
possible re-tune of threshold.

### 10. `PlaneMeshBuilder.subdivisions` split — TRIVIAL
None found in workspace. Skip unless a mesh builder is added.

### 11. Resources-as-components broad-query conflicts — LOW
Only bites `Query<EntityMut>` / `Query<Option<&T>>` style broad queries that
now alias resource entities. Add `Without<IsResource>` if the compiler flags a
conflict. Unlikely to hit us; fix reactively.

## Suggested sequence

1. Bump root `bevy` + all `bevy_*` sub-pins + feature-collection fixups. Bump
   `avian3d`→0.7, `bevy_egui`→0.40, `bevy-inspector-egui`→0.37,
   `bevy_replicon`→0.41, `lightyear`→0.28. Verify `leafwing` / `enhanced_input`
   / `transform_interpolation` exact 0.19 nums.
2. Feature-gate OFF `transform-gizmo-bevy` + `avian_pickup` (blockers) so the
   tree compiles; track upstream for 0.19 releases.
3. Mechanical sweeps: `rand` 0.10 imports, `TextFont` (1 site), `#[reflect(
   Resource)]` review, `Assets::get_mut` `mut`+guard.
4. Verify `bevy_scene`→`bevy_world_serialization` import/feature paths in
   `lunco-usd-bevy`.
5. `cargo check` per-crate, fix broad-query conflicts reactively.
6. Re-enable gizmo + pickup when upstream 0.19 lands; re-tune bloom.

## Open items to verify before starting
- ~~Exact 0.19 nums for leafwing / enhanced_input / transform_interp~~ — resolved:
  0.21.0 / 0.26.0 / 0.5.0 (all published on/after 0.19).
- `bevy_enhanced_input` repo moved `projectharmonia` → `simgine`; confirm the
  crates.io crate is unchanged and 0.26.0 targets bevy 0.19.
- Whether `avian3d` 0.7 vendors `bevy_transform_interpolation` (may drop our
  direct pin).
- `bevy_scene` runtime-`Scene` import path after the world-serialization split.
- Watch `big_space` #73 + `avian_pickup` #33 for merge/release; open a
  `transform-gizmo-bevy` 0.19 fork or upstream PR ourselves.
