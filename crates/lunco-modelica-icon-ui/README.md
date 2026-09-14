# lunco-modelica-icon-ui

Reusable egui renderer for authored Modelica `Icon` and `Diagram` graphics.

The package consumes parsed annotation data from `lunco-modelica-core`, applies
shared theme tokens, and resolves bitmap bytes through the MSL asset-source
boundary. It has no Modelica document, workbench panel, or application
lifecycle ownership, so diagram and preview UIs can depend on it independently.
