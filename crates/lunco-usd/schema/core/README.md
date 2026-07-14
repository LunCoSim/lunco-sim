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
