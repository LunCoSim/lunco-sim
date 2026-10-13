# PhysX extension declarations

`physxSchema.usda` retains the vehicle subset reconstructed from NVIDIA's
Omni Physics 107.2 schema reference. It is registered by OpenUSD's native
`SchemaRegistry` with the explicit `../physxManifest.usda` family manifest.
The native standard families come from `openusd-schemas`; no copied standard
schema registry is maintained here.

The PhysX subset is an existing project extension, not an official NVIDIA
schema distribution. Replace it with an authoritative distribution when
available and verify its consumed properties. Inspector hints use the shared
metadata decoder at build time. The native-definition test verifies registration
and the consumed suspension names; it does not establish vendor conformance.
