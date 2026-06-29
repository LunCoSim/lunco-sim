# lunco-obstacle-field

Procedural **crater + rock field** generation for rover testing.

Generates obstacle fields on the fly with tunable distribution parameters
(density, size distribution, spatial pattern, seed) so a rover can be tested
across varied surface conditions. Replaces the flat ground for mobility tests.

The generation core is **pure and deterministic** — the same `(spec, seed)`
always yields the same field. So networking replicates only the
`ObstacleFieldSpec`, and an experiment sweep just varies its numbers.

## Layers (modules)

| Module | Role |
|--------|------|
| `spec` | the tunable knobs (`ObstacleFieldSpec`, `Pattern`) |
| `sampler` | deterministic placement (ChaCha8, pure, off-thread-safe) |
| `field` | synthesised height surface: craters stamped as bowls, analytic `height_at` for raycast-free rock placement |
| `rock` | rock geometry |
| `assets` | size-bucket quantization (shared meshes/colliders) |
| `plugin` | the Bevy `ObstacleFieldPlugin` wiring it into the world |

## Key types

`ObstacleFieldPlugin`, `ObstacleFieldRoot`, `RegenerateField`,
`ObstacleFieldSpec`, `Pattern`; plus mesh helpers (`grid_mesh`,
`grid_indices`, `grid_normals`).

## Status

Working generator (server-authoritative colliders; client adds visuals). See
`PLAN.md` for the phased roadmap (streaming, dynamics, tuning UI, bake cache,
experiment sweep).
