# Core USD schemas — vendored, verbatim

OpenUSD's own `generatedSchema.usda` files. **Not ours, not edited.** Each is
copied byte-for-byte from an OpenUSD distribution:

| file             | OpenUSD module | source path                                  |
|------------------|----------------|----------------------------------------------|
| `usd.usda`        | `usd`          | `lib/usd/usd/resources/generatedSchema.usda`        |
| `usdGeom.usda`    | `usdGeom`      | `lib/usd/usdGeom/resources/generatedSchema.usda`    |
| `usdShade.usda`   | `usdShade`     | `lib/usd/usdShade/resources/generatedSchema.usda`   |
| `usdLux.usda`     | `usdLux`       | `lib/usd/usdLux/resources/generatedSchema.usda`     |
| `usdPhysics.usda` | `usdPhysics`   | `lib/usd/usdPhysics/resources/generatedSchema.usda` |

Licensed under the Apache License 2.0 (with OpenUSD's modifications notice) —
the same terms as the rest of OpenUSD.

## `physxSchema.usda` — reconstructed, not verbatim

Unlike the OpenUSD files above, `physxSchema.usda` is **reconstructed**, not
vendored. NVIDIA's PhysX `generatedSchema.usda` is not in the public
`github.com/NVIDIA-Omniverse/PhysX` repo (per NVIDIA staff on the developer
forum) — it ships only inside a Kit / Isaac Sim install at
`extsPhysics/omni.physx/schema.usda`, and no Kit install was available when this
file was written. It was reconstructed from the authoritative Omni Physics
schema reference (v107.2), which is generated from that exact file.

**Scope is deliberately minimal** — only the vehicle APIs this project uses
(`Context`, `AckermannSteering`, `TankDifferential`, `Wheel`, `Engine`,
`Suspension`) or needs per the suspension spec (doc 53: `WheelAttachment`,
`SuspensionCompliance`, `Tire`). NVIDIA's full vehicle family (~15+ APIs) is
larger; add those only when a feature consumes them.

**It has already been wrong once, and not in the way you would expect.** The
reconstruction contained `physxVehicleAckermannSteering:maxWheelAngleDegrees` —
a property that exists in no NVIDIA schema or document. It is not a misspelling
of a real property: it was *invented*, complete with a plausible unit that PhysX
does not use anywhere (PhysX is radians; only the Kit authoring wizard's UI field
is in degrees). The real name is `physxVehicleAckermannSteering:maxSteerAngle`.
A rover was nearly authored against the invented one.

So the risk here is not merely a wrong name for a real property — it is a
convincing name for a property that does not exist, which no amount of internal
consistency can detect. Treat every name in this file as unverified until the
verbatim swap. `PhysxVehicleSteeringAPI` (the non-Ackermann steering API, and the
documented replacement for the deprecated per-wheel `maxSteerAngle`) is missing
from this file entirely — an absence with the same cause.

**Replace with the verbatim file when a Kit install is available.** Copy
`extsPhysics/omni.physx/schema.usda` over this one. The drift test
`physx_vehicle_schemas_register_canonical_properties` in `../src/schema.rs`
pins the property names this codebase reads — including negative assertions for
the invented ones — so a verbatim swap is a verified replacement rather than a
guess. Note what that test can and cannot do: it catches a *swap* that drops or
renames something we read; it cannot tell you that a name we never questioned was
fabricated. Only the real file can.

## Why they're here

A property's **type**, its **variability** (`uniform`/`varying`) and whether it is
`custom` are declared by its *schema*, not by whoever authors it. `lunco_usd::schema`
is the one place that knows, and it used to know only *our* schema — core USD was a
hand-written table of ten properties someone had looked up, whose failure mode was
silence (author a core `uniform` property that isn't in the table and it gets written
`varying`, with no error).

The Rust `openusd` crate has no `UsdSchemaRegistry` to ask. But it doesn't need one:
**a `generatedSchema.usda` is just USDA, and we already parse USDA.** These files were
never unavailable — they simply weren't read. The registry now reads them with exactly
the parser it uses for `../generatedSchema.usda`.

## Updating

Copy the file from a newer OpenUSD, verbatim. Do not hand-edit: an edited copy is a
schema that disagrees with the USD everyone else is running, which is the whole class
of bug this directory exists to prevent.

Only the modules we author against are vendored. Adding another (`usdRender`,
`usdSkel`, …) is a copy plus one line in `CORE_SCHEMAS` (`../../src/schema.rs`) — no
code.
