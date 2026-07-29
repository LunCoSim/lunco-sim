# lunco-usd-compose

The render-free OpenUSD assembly leaf.

It receives authored layer bytes through `lunco-assets` canonical identities and
composes sublayers, references, payloads, and variants into an OpenUSD stage.
It has no Modelica, Rhai, behavior-tree, physics, Bevy entity, or rendering
responsibility.

For file listings and scenario manifests, it exposes `is_usd_layer` and
`layer_dependency_arcs`. `lunco-assets::transitive_file_closure*` consumes those
format facts and owns the actual filesystem traversal; this crate does not own a
second closure walker.
