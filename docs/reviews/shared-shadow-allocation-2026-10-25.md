# Native shared-shadow allocation

The render adapter composes Bevy 0.20's native `allocate_uniforms` before
`unpack_bins` in `SharedShadowView`. Point and spot light roots use this schedule
independently of camera and directional-cascade preparation. The native kernel
establishes each batch's output mesh-uniform offset and clears indirect instance
counts before GPU preprocessing. Omitting it leaves valid authored local lights
without the expected shadows. No GPU algorithm, allocator, material ABI, quality
setting or simulation behavior is reimplemented or changed.

The initial investigation targeted the native GPU preparation costs in the
qualified settled capture: approximately 2.075 ms for preprocess bind groups and
1.265 ms for shadow instance-buffer preparation. The
[native builder comparison](native-instance-builder-2026-10-25.md) uses the
complete GPU path qualified here. Rendering without the required shadows cannot
establish an equivalent performance result.

## Verification

The strengthened schedule test checks native system identities and execution
order, plus the absence of GPU stages in CPU mode and the existing deferred-flush
contract. It fails before adding allocation and passes after:

```sh
cargo test --locked -p lunco-render-bevy --lib \
  shadow_view_schedule::tests::shared_schedule_retains_native_gpu_admission_and_final_buffer_flush \
  -j 4 -- --nocapture
```

Two obsolete test API uses prevented this target from compiling. Shape primitives
now import from `bevy::shape`, and the removed `ExtractedCamera.compositing_space`
field is deleted. Optional debug system names are not used for ordering checks.

The new `shared_shadow_allocation` scene is authored and read back through the
production document API. It uses a point light, two differently sized caster
meshes, a receiver, and all three native camera prepasses. Rhai verifies the
native light and prepass setup; the GPU probes independently require a lit
vacated location and a shadow at the displaced caster. Existing vertex-deformation
shader logic is reused. The shared render runner checks both positive colors
and rejects red disagreement pixels.

The baseline's four authored setup checks pass, but its pixel gate exits one:
107 green, zero cyan and 107 red pixels in the 320 by 180 frame. The corrected
normal binary produces 107 green, 107 cyan and zero red pixels. The qualified
runner records owned PID 2005330/API 5242, verifies its cwd and exact production
executable, drains three frames and exits zero. Both images are inspected.
Earlier incomplete fixture setup and a coordinator PID-regex failure are excluded;
the final recorded process identity and terminal status cover the passing gate.

`cargo build --locked -p lunco-luncosim -j 4` passes in 1m16s. Normal executable
SHA-256 is `d227f3cec564c2c08b48886ba0e2b244388ac3cb9e24e639679d33ad7ea0f187`.

Normal Griffin PID 2006104/API 5243 completes touchdown and four-pad handoff,
five seconds of full terrain readiness and at least 240 fresh measured ticks.
The image retains the landed vehicle, terrain and shadow; registered diagnostics
are clear and the app closes. All 271 source hashes match the preceding run and
remain unchanged. First physics readiness is 16.151 s, landed eligibility
130.667 s, coupled median/p95 5.681/9.255 ms and frames 80.348/99.374 ms.
Concurrent Tesla rendering and 2560 by 1568 video encoding qualify this window.
The preceding source-matched run has 6.397/7.675 ms coupled service and
22.185/26.517 ms frames under different host pressure. These do not isolate a
performance effect of the allocation stage. All original targets remain open.

Raw images, logs, identities and measurements stay ignored under
`target/scene-tests/` and `.cache/codex-evidence/performance-20261023/`, in
`shared-shadow-*`, `griffin-native-builder-gpu-before/` and
`griffin-shared-shadow-allocation-normal/`. The temporary baseline executable
hard link is removed; normal Cargo outputs and shared caches are retained.
