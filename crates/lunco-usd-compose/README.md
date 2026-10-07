# lunco-usd-compose

The render-free OpenUSD assembly leaf.

It receives authored layer bytes through `lunco-assets-core` canonical identities and
composes sublayers, references, payloads, and variants into an OpenUSD stage.
It has no Modelica, Rhai, behavior-tree, physics, Bevy entity, or rendering
responsibility.

For file listings and scenario manifests, it exposes `is_usd_layer` and
`layer_dependency_arcs`. `lunco-assets-core::transitive_file_closure*` consumes those
format facts and owns the actual filesystem traversal; this crate does not own a
second closure walker.

Native file entrypoints canonicalize the native root through `lunco-storage`
and encode it as a standard file URI. `canonicalize_at` returns `Result` and
anchors literal sibling filenames with URL path segments, preserving Windows
drive, UNC, and verbatim roots. Authored native Windows drive/UNC references
normalize through `lunco-storage::windows_file_reference_uri`; drive-relative
paths and foreign-host Windows filesystem references produce explicit errors. Asset-root references (`/…`) keep their logical
asset meaning. The native asset reader decodes file identities through storage;
invalid file URIs are terminal errors. `LuncoUsdResolver::new` validates layer
identifiers and composition arcs before admission; stage builders also check
its diagnostic handle after the infallible OpenUSD callbacks.
