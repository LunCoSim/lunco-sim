# lunco-core

Shared ECS contracts and engine facts for LunCoSim. This crate is deliberately
below application composition and domain packages.

It owns stable identity/provenance, shared marker components, typed scene
requests, runtime diagnostics/fault contracts, generic state markers, and
small ECS utilities. Command reflection is still owned by the typed command
surface exposed here; domain handlers remain in their owning crates.

The following responsibilities are intentionally outside this crate:

- fixed-step/runtime scheduling lives in `lunco-core-runtime`;
- reconciliation math lives in `lunco-networking-core`;
- recoverable mutex access lives in `lunco-core-runtime`;
- typed exposure storage lives in `lunco-exposure-core`, while the projection
  plugin remains in `lunco-luncosim-exposures`;
- BigSpace, physics, USD, Modelica, avatar, and UI composition live in their
  respective domain/application packages.

This ownership split keeps changes to those high-churn mechanisms from
rebuilding every consumer of the stable engine contracts.

## See Also

- [Workbench architecture](../../docs/architecture/11-workbench.md) — current UI and document-view decisions
- [Engineering Ontology](../../docs/architecture/01-ontology.md) — engineering terminology source of truth
