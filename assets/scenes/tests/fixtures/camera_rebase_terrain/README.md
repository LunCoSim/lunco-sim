# Camera origin-rebase terrain fixture

The 257 × 257 GeoTIFF stores constant float64 elevations of -1918 m over a
4000 m square centred at the origin. Pixel scale is 15.625 m; the tie point is
(-2000, 2000). Lunar GeoKeys match the maintained flat camera fixture.

The referencing scene starts the avatar at x = 995 m so rightward motion
crosses the WorldGrid origin-cell boundary. The scene is authored through the
production USD document API. The raster uses lossless Deflate compression.
