# lunco-workbench-datasets-ui

Optional dataset controls for the LunCoSim workbench browser.

The generic `lunco-workbench-browser` package does not depend on the archive,
HTTP, raster, or SVG processing closure in `lunco-assets`. Hosts that need to
show and request resources declared by the active Twin add
`TwinDatasetsPlugin` from this package.

Dataset identity, manifest interpretation, authorization, download, and local
processing remain in `lunco-assets`; this package only projects the shared
registry into the browser and emits its typed commands.
