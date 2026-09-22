# Research Note: High-Quality Lunar Surface Rendering

> Status: Research · Audience: engineers building real-time lunar terrain, materials, and camera simulation

This note synthesizes public NASA, NASA JPL, Open Robotics, and computer-graphics references on rendering the Moon. It focuses on methods that produce detailed, stable terrain at rover scale while retaining believable views over larger distances. It separates measured data, synthetic terrain, material shading, and camera processing because each solves a different part of the image.

## Main finding

High-quality lunar imagery is a coordinated terrain-and-rendering pipeline. The strongest documented systems combine registered elevation and image data, scale-aware terrain geometry, statistically constrained terrain enhancement, lunar-specific reflectance, detailed near-field shadows, filtered material microdetail, and suitable exposure. A shader cannot recover shape that the elevation data does not contain; a high-resolution image map improves surface colour but does not provide matching silhouettes, slopes, occlusion, or cast shadows.

The required representation depends on the use case. A cinematic close-up can use plausible authored detail. A rover-driving or perception simulation must preserve the shape, feature distribution, illumination, and camera characteristics that matter to navigation. Those fidelity claims should remain separate.

## 1. Build the terrain from data at the intended scale

### Elevation, imagery, and georeferencing have different roles

Use stereo-derived topography and laser-altimeter control for shape; use calibrated, map-projected imagery for surface colour. LRO products include stereo-image topography at roughly 0.5–2 m/pixel and laser-altimeter measurements. The datasets are complementary: imagery resolves small visible features, while LOLA measurements supply a geodetic framework and useful heights where imaging is limited.[1]

Keep elevation and image data co-registered in one map projection and physical scale. A terrain texture offset from its DEM produces doubled rims and inconsistent shadows, even when both inputs are individually high resolution. Preserve source resolution and uncertainty through cropping and resampling; do not present interpolation as new measured detail.

LROC mosaics are radiometrically calibrated, photometrically normalized to standard illumination conditions in most products, and resampled to a standard projection. The LROC product specification also warns that basemaps can be geodetically uncontrolled and limits their positional accuracy accordingly.[2] For visually stable materials, derive intrinsic albedo from calibrated and appropriately normalized imagery rather than treating arbitrary display imagery as reflectance. Keep baked illumination separate from albedo whenever the simulation must change the Sun direction.

### Make mesh resolution follow the camera and task

NASA Ames and Open Robotics’ lunar rover simulator divided its terrain into a high-resolution drivable region and progressively lower-resolution background tiles. Its paper describes roughly 3–5 cm postings in the detailed driving area, while the background uses simpler meshes toward the horizon. It also describes terrain chunking to mitigate LOD popping and emphasizes correct UV transforms across chunks.[3]

This is the central performance technique: spend triangles and detailed material work where the camera or vehicle can resolve them, and use coarser representations at distance. Keep neighboring resolutions visually continuous through controlled LOD transitions, consistent texture coordinates, and suitable overlap or geomorphing. Cache processed terrain tiles or heightmap data so raster conversion and mesh construction do not repeat at each launch. The 2017 Gazebo report attributes better terrain performance and much faster subsequent loads to LOD and on-disk heightmap caching.[4]

### Enhance only the scales missing from measured elevation

Rover-scale simulations often need centimetre-scale detail even when the best orbital DEM is measured in metres. NASA’s rover-simulator paper describes fractal elevation synthesis together with crater and rock populations based on lunar size-frequency models.[3] NASA’s later Lunar Surface Simulation presentation frames rover terrain as needing about a 20-fold finer linear resolution than a 1 m DEM, then lists fractal expansion, enhancement of craters visible in orbital imagery, compensation for low-resolution smoothing, estimating crater depth from shadow volumes, and adding smaller craters and rocks below the source-data threshold.[5]

NASA JPL’s LuNaSynth follows the same broad structure: start from a lunar DEM, add crater and rock fields, then render the terrain at a chosen target scale. Its documented target detail ranges from metre scale down to centimetres, with rocks placed from a distribution model and craters represented by parametrized terrain modifications.[6]

The general lesson is to synthesize a **missing spatial-frequency band**, not to add arbitrary noise on top of all measured frequencies. Preserve the source DEM’s low-frequency form; infer or generate only unresolved scales; use crater and rock statistics appropriate to the terrain and target use; and keep the realization deterministic and continuous across tiles. A normal or bump map can add fine shading cues, but cannot create the silhouette, parallax, or cast shadow of a geometric feature.

## 2. Shade lunar regolith as a particulate surface

### Use reflectance that responds to Sun, camera, and surface orientation

The lunar surface is particulate regolith, not a generic matte sphere. Its measured brightness depends on incidence angle, emission angle, phase angle, particle scattering, surface roughness, and opposition effects. Hapke’s bidirectional reflectance model is widely used because it represents those effects, including brightening near opposition and the roles of roughness and particle scattering.[7]

The full Hapke model can be expensive and can produce edge artifacts in real-time rover-scale rendering. The NASA Ames/Open Robotics simulator therefore used a practical approximation: combine shadow-hiding and coherent-backscatter opposition effects into a compact term, then combine that approximation with a Lambertian response adjusted by viewing geometry to suppress artifacts.[3] This supports choosing a validated approximation when it is more robust for the target renderer; it does not support replacing lunar photometry with plain Lambert shading.

A useful progression for a real-time material is:

1. Start with a lunar-Lambert or Lommel–Seeliger/Lambert blend for the dominant diffuse response.
2. Add a bounded, fitted opposition term if the target phase-angle range needs it.
3. Add Hapke’s richer particle, roughness, and multiple-scattering terms when image or sensor fidelity justifies the cost.
4. Fit or select parameters from lunar observations over a range of lighting and viewing geometries. Spatially resolved lunar Hapke parameter maps demonstrate that one global parameter set need not describe every region equally well.[7]

Hapke is not a single visual preset: parameter values and phase-angle coverage matter. A newer lunar ground-operations simulator uses Hapke photometric functions with ray-traced lighting and reports that strong opposition brightening can wash out detail when the camera and light align. It also notes that high-resolution particle-scale geometry naturally creates fine shadow structure; normal and shadow maps are the usual raster-rendering approximation for effects too small to model explicitly.[8]

An emerging alternative is to estimate spatially varying BRDF parameters from data. Lunar-G2R (2026) predicts reflectance parameters from a DEM using a U-Net trained with differentiable rendering against real orbital images and known illumination/view geometry. Its abstract reports a 38% photometric-error reduction over its baseline on a geographically held-out Tycho region.[12] This is promising for offline material fitting, but it is learned appearance rather than a replacement for topographic geometry or a universally validated lunar soil model; the reported region and training assumptions matter.

### Separate material colour from directional lighting

Treat albedo as a surface property and sunlight, terrain shadows, and exposure as lighting or camera properties. If source images contain their acquisition shadows or illumination gradients, feeding them directly into diffuse albedo bakes one lighting solution into the material and then lights it a second time at runtime. Photometric normalization, orthorectification, and albedo/terrain separation help avoid this double-lighting problem.[2, 9]

An illumination or hillshade layer can be useful for a fixed reference image, a diagnostic overlay, or a deliberately baked presentation. For a scene with a moving Sun, it must not silently replace live topographic lighting. Keep colour, roughness, normal, height, and any static visibility data in well-defined channels with explicit linear/sRGB handling.

### Match lunar dynamic range and camera response

Lunar scenes combine strong direct sunlight, deep terrain shadows, a nearly black sky, and little atmospheric fill. NASA’s rover simulator treats high dynamic range, Sun/Earth geometry, shadows, and camera exposure as explicit parts of its visual simulation; its early public rendering account also describes lens-flare post-processing as a camera effect.[4, 8, 10]

Use a physically consistent Sun direction and intensity, restrained indirect fill, and a camera exposure/tone-mapping path that preserves both bright regolith and deep shadow detail. Apply sensor artifacts—flare, noise, blur, distortion, or compression—after material and lighting evaluation, and only when the target camera or presentation requires them. Camera effects should not disguise a poorly lit or incorrectly calibrated surface.

## 3. Add close detail without aliasing

Organize appearance into distinct scales:

- **Landform:** measured or carefully enhanced DEM geometry; owns large slopes, crater silhouettes, and long cast shadows.
- **Mesoscale:** resolvable rocks, small craters, block fields, and ejecta; use geometry or displacement when their silhouettes and shadows matter.
- **Fine surface:** regolith grain and roughness; use normal/bump or a filtered microfacet/normal-distribution representation.
- **Subpixel surface:** replace individual features with their filtered statistical shading response instead of drawing unstable speckles.

NVIDIA’s real-time multiscale-material work addresses the final two scales. It estimates pixel footprints at multiple levels, evaluates a hierarchy of detail, and filters subpixel microstructure for temporal stability and antialiasing. The transferable lesson for regolith materials is that detail should fade or transform as pixel footprint grows; distant microgeometry should affect aggregate shading rather than remain visible as high-frequency noise.[11]

Use texture mipmaps and anisotropic filtering for image detail, and use screen-space or ray-differential footprints to choose procedural frequency and amplitude. Keep procedural coordinates stable in terrain/world space to avoid detail swimming as the camera moves. When bumps become unresolved, filter their normal distribution or shift their energy into roughness; do not simply increase bump strength. Use geometry only for details whose silhouette, parallax, contact, or cast shadow must be visible.

## 4. Spend shadow quality where it changes the image

Real-time shadows are an important part of lunar shape perception. Gazebo’s lunar work increased shadow texture resolution and tuned shadow parameters specifically for sharper terrain shadows, while terrain LOD bounded the cost of rendering high-resolution terrain.[4] Shadow-map resolution alone is not a quality solution: it must be paired with appropriate cascade placement, stable biasing, filtered sampling, and geometry whose detail is supported by its source data or synthesis model.

Prioritize the near-field cascade for rover-scale contact and rock/crater detail. Keep farther cascades broad enough for major landforms and route visibility. Tune against low solar elevations as well as high Sun, because shallow light creates long shadows that amplify small height errors and aliases. If illumination changes dynamically, render dynamic shadows from geometry; use precomputed illumination only as an explicit static contribution.

## 5. A practical high-quality, real-time recipe

1. **Define the target image scale.** Choose the close range, field of view, output resolution, and smallest feature the camera must resolve.
2. **Assemble and register source layers.** Use the best suitable DEM, image/albedo, geodetic control, and camera/Sun metadata. Record each layer’s resolution and uncertainty.
3. **Build a tiled LOD terrain.** Keep a high-detail active region around the vehicle and coarsen toward the horizon. Preserve UVs and ensure transitions do not pop.
4. **Fill only unsupported detail bands.** If enhancement is necessary, preserve measured landforms and use validated crater/rock size distributions and morphology. Make synthetic detail deterministic, continuous across tile boundaries, and distinguishable from measured terrain in data provenance.
5. **Use each material channel for one role.** Intrinsic colour belongs in albedo; fine orientation in normals; height in displacement/bump; unresolved normal variation in filtered roughness or a microfacet distribution; static illumination only when lighting is fixed.
6. **Apply lunar photometry and controlled shadows.** Start with a robust lunar-Lambert/Hapke approximation and tune against known geometries. Use a near-focused shadow budget and preserve the correct direct-Sun direction.
7. **Render with the target camera.** Apply exposure and optional sensor effects after lighting, not as substitutes for surface or terrain data.
8. **Validate across scales and illumination.** Compare close, midrange, and horizon views; test camera motion for shimmer and LOD popping; compare low and high solar elevations and multiple phase angles; measure GPU time, memory, and terrain load time separately from appearance.

## Technique selection

| Technique | Best contribution | Main cost or limitation |
|---|---|---|
| Stereo/altimeter DEM | Correct landform, silhouette, and terrain shadows | Resolution, coverage, and registration limit detail |
| Orthorectified, normalized imagery | Real surface colour and recognizable local texture | Does not add shape; may retain photometric or geometric error |
| Statistical crater/rock synthesis | Plausible hazards below source-DEM resolution | Requires terrain-appropriate distributions and validation |
| Normal/bump maps | Cheap fine shading detail | No true silhouette, parallax, contact, or geometric cast shadow |
| Displaced or instanced detail geometry | Correct local silhouettes and shadows | Vertex, draw-call, memory, and LOD management cost |
| Hapke or lunar-Lambert BRDF | Moon-specific angular reflectance | Parameter fitting and shading cost; approximations need validation |
| Pixel-footprint-filtered microdetail | Stable detail at changing distance and resolution | Requires frequency-aware evaluation and a filtered far-field response |
| Tiled LOD and cached terrain | High detail near, efficient far terrain and repeated startup | Seam, UV, and transition quality must be managed |
| Baked illumination | Low runtime cost for fixed lighting | Incorrect when the Sun or lighting changes |

## References

1. NASA GSFC Planetary Geodesy and Altimetry, [High-resolution Lunar Topography (SLDEM2015)](https://pgda.gsfc.nasa.gov/products/54) — LOLA coverage, precision, and its geodetic role alongside SELENE stereo topography; NASA Science, [LRO science and data](https://science.nasa.gov/mission/lro/science-and-data/) — stereo imagery at 0.5–2 m/pixel.
2. LROC Science Operations Center, [RDR SIS specification](https://pds-imaging.jpl.nasa.gov/documentation/LROC_SOC_RDR_SIS_Spec_4_v1-2.pdf) — calibration, photometric normalization, map projection, and product limitations.
3. M. Allan et al., [“Planetary Rover Simulation for Lunar Exploration Missions”](https://doi.org/10.1109/AERO.2019.8741780), IEEE Aerospace Conference, 2019; [NASA NTRS PDF](https://ntrs.nasa.gov/api/citations/20190027571/downloads/20190027571.pdf).
4. Open Robotics, [“Gazebo renders the moon”](https://www.osrfoundation.org/gazebo-renders-the-moon/), 2017.
5. M. Allan, [“Lunar Surface Simulation”](https://ntrs.nasa.gov/citations/20250001422), NASA Ames presentation, 2025; [PDF](https://ntrs.nasa.gov/api/citations/20250001422/downloads/OR-RSIMLunarSurfaceSimulation.pdf).
6. NASA JPL, [LuNaSynth: Synthetic Generation of Lunar Terrain](https://github.com/nasa-jpl/lunasynth).
7. H. Sato et al., [“Resolved Hapke parameter maps of the Moon”](https://doi.org/10.1002/2013JE004580), *Journal of Geophysical Research: Planets*, 2014.
8. N. M. Batagoda et al., [“A physics-based sensor simulation environment for lunar ground operations”](https://arxiv.org/abs/2410.04371), 2024.
9. NASA, [Lunar terrain and albedo reconstruction from Apollo imagery](https://data.nasa.gov/dataset/lunar-terrain-and-albedo-reconstruction-from-apollo-imagery).
10. T. Fong, [“Lunar Robotics Update”](https://ntrs.nasa.gov/api/citations/20190001347/downloads/20190001347.pdf), NASA Ames presentation.
11. T. Zirr and A. Kaplanyan, [“Real-time Rendering of Procedural Multiscale Materials”](https://research.nvidia.com/publication/2016-02_real-time-rendering-procedural-multiscale-materials), I3D 2016; [paper PDF](https://research.nvidia.com/sites/default/files/pubs/2016-02_Real-time-Rendering-of/ZirrKaplanyan_MultiscaleI3D2016.pdf).
12. C. Grethen et al., [“Lunar-G2R: Geometry-to-Reflectance Learning for High-Fidelity Lunar BRDF Estimation”](https://arxiv.org/abs/2601.10449), arXiv, 2026.

## Evidence limits

The NASA 2019 and 2025 materials describe engineering systems and methods, not a universal quality benchmark. LuNaSynth’s README documents its pipeline at a high level; its suitability for a particular site depends on its input data and distribution parameters. The NVIDIA multiscale paper addresses general procedural materials, so its filtering principles transfer to lunar regolith but are not themselves lunar reflectance validation. Use source imagery, photometric parameters, and terrain statistics appropriate to the intended region and camera.
