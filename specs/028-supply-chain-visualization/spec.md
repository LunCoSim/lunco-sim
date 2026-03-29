# Feature Specification: 028-supply-chain-visualization

**Feature Branch**: `028-supply-chain-visualization`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Macroscopic Logistics, SysML network mapping, and Global Supply/Demand visualizations.

## Problem Statement
While `025-isru-resource-economy` handles the local mechanics of specific factories cracking regolith into oxygen, mission planners need a macroscopic view of the *entire planetary supply chain*. The engine must provide a visualization layer that parses the overarching `013-sysml-integration` logistics network graph, mapping resource flows between distant outposts (Earth -> Lunar Gateway -> South Pole Base).

## User Scenarios

### User Story 1 - SysML Node-Link Logistics Graph
As a logistics engineer, I want to see a 2D/3D node-link overlay showing all my active supply chain routes as defined by the SysML architecture.

**Acceptance Criteria:**
- The Unified Editor (`007`) provides a "Logistics Macro-View" mode.
- Global entities (like entire orbital stations or ground bases) are summarized as distinct nodes.
- Animated edges (lines) visualize the macro-rate and volume of resource transfer (e.g., Cargo shipments arriving once a month) between nodes.

### User Story 2 - Planetary Bottleneck Analysis
As a mission manager, I want to see which outposts are resource-starved over long timescales across my entire lunar infrastructure.

**Acceptance Criteria:**
- The logistics view highlights macroscopic nodes in red if their aggregated total inventory is "Starved" or "Storage Full".
- Tooltips provide aggregated telemetry without needing to teleport the 3D camera to the base.

### User Story 3 - Global Supply/Demand Dashboard
As a commander, I want a high-level dashboard summarizing total planetary production and consumption.

**Acceptance Criteria:**
- A dashboard aggregates the production (`d(Oxygen)/dt`) and consumption (`d(Power)/dt`) metrics from all active `FactoryNode`s across the entire simulation world.
- Total "Planetary Stock on Hand" is visualized against predicted demand curves.
