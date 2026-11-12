# Flat DEM camera navigation fixture

The 257 × 257 GeoTIFF contains constant f64 elevations of -1918 m. It uses
the lunar metric georeferencing of the maintained rocks fixture, with a
1000 m square footprint. The compressed raster is repository owned.

`camera_flat_terrain.usda` places its free camera 120 m from a native rigid
body. The collider ring follows that body; camera navigation must use the
source and its own presented terrain cover. The flat corridor makes an
unrequested altitude change distinguishable from sliding on a real slope.

The scene was authored and saved through the production USD document API.
The raster was generated with `tifffile` from an f64 constant array, preserving
the reference raster's pixel scale, tie point and lunar GeoKeys.
