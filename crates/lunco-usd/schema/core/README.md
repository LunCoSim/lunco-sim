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

**This is a RECONSTRUCTION, and its risk is not a wrong name for a real property
— it is a convincing name for one that does not exist.** No amount of internal
consistency detects that. A worked example lives in the drift test as a negative
assertion: `physxVehicleAckermannSteering:maxWheelAngleDegrees` is in no NVIDIA
schema or document (PhysX steering is radians; the real property is
`maxSteerAngle`), yet it reads as plausible. Treat every name in this file as
unverified until the verbatim swap, and prefer authoring against a name you have
confirmed against NVIDIA's docs. `PhysxVehicleSteeringAPI` — the non-Ackermann
steering API and the documented replacement for the deprecated per-wheel
`maxSteerAngle` — is absent from this file entirely, for the same reason.

**Replace with the verbatim file when a Kit install is available.** Copy
`extsPhysics/omni.physx/schema.usda` over this one. The drift test
`physx_vehicle_schemas_register_canonical_properties` in `../src/schema.rs` pins
the property names this codebase reads, including negative assertions for the
fabricated ones — but note its limit: it catches a *swap* that drops or renames
something we read; it cannot tell you a name we never questioned was fabricated.
Only the real file can.

## Why they're here

A property's **type**, its **variability** (`uniform`/`varying`) and whether it is
`custom` are declared by its *schema*, not by whoever authors it. `lunco_usd::schema`
is the one place that knows, and it must know core USD too — otherwise a core `uniform`
property it hasn't been told about gets written `varying`, with no error.

The Rust `openusd` crate has no `UsdSchemaRegistry` to ask, and does not need one:
**a `generatedSchema.usda` is just USDA, and we already parse USDA.** The registry reads
these files with exactly the parser it uses for `../generatedSchema.usda`.

## Updating

Copy the file from a newer OpenUSD, verbatim. Do not hand-edit: an edited copy is a
schema that disagrees with the USD everyone else is running, which is the whole class
of bug this directory exists to prevent.

Only the modules we author against are vendored. Adding another (`usdRender`,
`usdSkel`, …) is a copy plus one line in `CORE_SCHEMAS` (`../../src/schema.rs`) — no
code.
