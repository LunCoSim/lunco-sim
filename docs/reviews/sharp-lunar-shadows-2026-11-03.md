# Close lunar shadows

The orbital Moon material tests the elevation texture for terrain occlusion.
Normal mission views also need native shadow maps to resolve vehicle geometry,
vehicle shadows on terrain, and terrain shadows on vehicles. The existing High
preset used 4096-pixel directional maps, Gaussian 5×5 filtering, 0.03 m depth
bias and 4.0-texel normal bias. This broad filter and conservative offsets hide
small close-up shadow detail and separate it from its caster under low Sun.

The High policy now selects 8192 pixels, Hardware 2×2 filtering, 0.005 m depth
bias and 2.0-texel normal bias. Four cascades retain the existing 30 m first
bound and 3000 m maximum. The logical byte ceiling is explicitly 5 GiB to cover
the unchanged two directional, eight point and eight spot caster limits:
spots use the directional atlas resolution. This is not a request to allocate
all caster slots or a claim about physical GPU memory. No lower-quality
fallback, Sun rotation/intensity/angle, terrain height or physics change is used.
Balanced and Low are unchanged. Existing custom settings remain authoritative;
select High in Graphics to apply its new defaults.

A visible 2560×1440 mission view (API 4493, PID 2812313) reports 8192 pixels,
four live cascade layers and 1,073,741,824 logical bytes for one Sun. There are
no RuntimeDiagnostics errors or warnings. Native light readback confirms
0.005 m depth bias and 2.0 normal bias. The new broll/headful-11 take contains
720 images paired individually with native simulation ticks. Close wheel,
chassis and solar-frame shadows are sharper and terrain remains visible.

Evidence under Griffin V13 R4 renders/evidence:
rover-shadow-before-r4.png, rover-shadow-high8k-native-r4.png,
orbital-and-broll-r4-frame-gate.json. These static views use different scene
runs; do not treat them as a pixel-perfect same-pose A/B test. Earlier manual
8K UI attempts without a sufficient ceiling were correctly rejected; they are
not accepted 8K evidence. Moving Sun, lander, engine-off impacts, solar turn and
route sources were re-recorded at this profile. All encoded image indices in
the accepted High takes were paired with native simulation ticks, including
the held result and camera orbit. FPS has not been qualified by this check.

Primary renderer reference:
https://docs.rs/bevy/latest/bevy/light/struct.DirectionalLight.html

The user authorized local work without Trello. Nothing has been pushed.
