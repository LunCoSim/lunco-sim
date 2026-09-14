# lunco-workbench-browser

Reusable Twin and Files navigation for LunCoSim workbench applications.

This package is an optional feature layer over `lunco-workbench`. It owns the
standard `TwinBrowserPanel` and `FilesPanel`, browser query/action resources,
and filesystem and library sections. Domain UI crates contribute their own
`BrowserSection` implementations without modifying the shell or depending on
the private dock state. Dataset controls are supplied by the separate
`lunco-workbench-datasets-ui` package.

## Usage

Install the browser feature after the concrete shell:

```rust,no_run
use bevy::prelude::*;
use lunco_workbench::WorkbenchPlugin;
use lunco_workbench_browser::TwinBrowserPlugin;

App::new()
    .add_plugins(WorkbenchPlugin)
    .add_plugins(TwinBrowserPlugin);
```

The browser package depends only on the lightweight `lunco-assets-core`
contract and can therefore be reused without the archive, HTTP, raster, or SVG
processing stack. Hosts that want Twin dataset controls add
`lunco-workbench-datasets-ui`, which owns that opt-in `lunco-assets` edge.

Domain sections use `BrowserSectionRegistry` and emit typed `BrowserAction`
values through `BrowserActions`. Browser panels consume `WorkspaceResource`;
they do not open USD layers or read asset bytes directly.

See [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md)
for the ownership and composition rules.
