# Vendored OpenUSD

Source: https://github.com/LunCoSim/openusd, revision e388c0edb1f350d8c02e1fa37294e680dda5b2dd.
The package source, tests, examples and local fixtures are retained. External
upstream `vendor/` test assets and documentation are not packaged. License: LICENSE.

Local extension: SdfAssetPath records a derived, nonserialized canonical asset
identifier produced by the existing strongest-default opinion resolver. This
preserves layer anchoring even when the external payload has not been opened.
Authored value equality, serialization, expression evaluation and composition
strength remain maintained OpenUSD behavior. Time-sampled asset provenance is
not inferred; consumers reject a missing canonical identifier.

The package is an explicit workspace member so owner-level tests use the normal
workspace target and lockfile. It retains the maintained third-party edition
2021; repository-owned Rust packages use edition 2024. The focused serialization
seam is `sdf::asset_path::tests::derived_identifier_not_serialized` under
`cargo test -p openusd --lib --features serde`. The omitted external corpus is
not required by this inline unit test; do not substitute a full upstream suite.
