# Path-traced scene with native transparent effects

Status: user approved on 23 October; implemented and production debug build
passed. Bounded native windowed and offscreen hybrid fixtures pass; actual
Roadster content qualification is pending.

## Required ordering

The pinned Bevy 0.20 reference path tracer runs after the opaque and transparent
main passes and before tonemapping. Its final color overwrites the transparent
pass. The trace function lives in a private module and has no public ordering
set. The public `main_transparent_pass_3d` can be reused, but the application
cannot currently place a pass after tracing and before tonemapping through a
supported scheduling boundary.

[Prepared patch](pathtracer-forward-composition.patch) adds one public system
set to the exact current `bevy_solari` source revision
`a12e8366cdd9c0ee8dabd3ada7bb7e8f02f0b338`. It changes ordering metadata only;
no shader, ray integrator, light model or physical simulation changes.

## Application integration

Only the pinned `bevy_solari` crate is vendored, with its original licences and
this patch. Its package manifest uses the existing Bevy 0.20 dependencies.
The Cargo cache is unchanged and no fork was published.

In `lunco-render-bevy`, an application-owned wrapper invokes the public
`main_transparent_pass_3d`. The wrapper checks that its render-world view has
`Pathtracer`; raster and preview views return immediately. It runs after
`PathtracingSystems::Trace` and before early post-processing, including bloom
and tonemapping. The wrapper forwards to
Bevy's pass rather than copying its rendering implementation. A distinct wrapper
system identity avoids ordering the original main-pass instance after itself.

Path accumulation contains the opaque result only. The native transparent pass
composites that image's current effects after accumulation, once per render
submission; it must not blend repeatedly into the accumulation texture. Existing
opaque depth provides primary-view occlusion. This is hybrid rendering: jets
remain forward-rendered and are not ray-traced volumes or secondary reflections.

## Remaining admission boundaries

This patch alone does not qualify the Roadster film. Unsupported opaque custom
shaders and local lights remain explicit errors. Only transparent surfaces that
the native forward pass actually supports may enter the hybrid path. Point,
spot and rectangle lighting require a supported implementation or deliberate
authored emissive lighting; none may disappear silently. The default raster
renderer remains unchanged.

## Required proof before production capture

- Check the patched dependency and application through the normal debug build.
- Native fixture: opaque occluder, moving transparent effect, emissive source,
  and a path-traced reflection. Confirm occlusion, no double blending, and no
  effects left behind after reset or backend changes.
- Negative fixtures: unsupported opaque shader and local light reject capture.
- Check independent raster, preview and path-traced views and Twin replacement.
- Verify native 60 Hz receipts and image sampling acknowledgements while one
  physical state is held for multiple render submissions.
- Review actual Roadster frames, including jets, wheels, body material and all
  stage lighting. Only then select this backend for full production takes.
