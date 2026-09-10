# Terrain shader appearance fixture

This is a small, repository-owned processed DEM used by the production graphics
scene `assets/scenes/tests/terrain_shader_appearance.usda`. It is deliberately
separate from downloaded Twin data so terrain appearance checks do not depend on
an external cache or network state.

The raster is a 512 × 512 float32 GeoTIFF with a centred lunar metric
geotransform. Its authored footprint is approximately 1002 m square, which
gives the capture camera distinct near, middle, and far relief bands while
keeping the fixture small enough for local and CI runs.

Heights remain absolute for the DEM contract (approximately -1999 to -1909 m);
the scene camera is authored near that datum rather than at elevation zero.

The scene uses the normal USD material binding and the production
`terrain_geomorph.wgsl` path. No Rust-only selector or test renderer is involved.
