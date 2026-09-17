# lunco-usd-viewport-runtime

This crate owns the render runtime for document-backed USD preview sessions.
It creates isolated preview scene roots, cameras, lights, render targets, and
projection-readiness state, and registers the typed preview commands and
inspection queries.

The render-independent session and view contracts remain in
`lunco-usd-viewport-core`. The egui workbench panels are installed separately
by `lunco-usd-viewport-ui`, so a runtime host can use preview lifecycle and
render resources without linking the panel registration surface.
