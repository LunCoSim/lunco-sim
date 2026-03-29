# Feature Specification: 028-supply-chain-visualization

**Feature Branch**: `028-supply-chain-visualization`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Requirements for high-level logistics and supply chain monitoring.

## Problem Statement
While Spec 025 (ISRU Resource Economy) handles the math of logistics, we need a way to visualize the macroscopic flow of materials between distant entities (e.g., Earth -> Lunar Station -> South Pole Base). A purely 3D-camera view makes it difficult to understand bottlenecks in a multi-node supply chain.

## User Scenarios

### User Story 1 - Node-Link Logistics Graph
As a logistics engineer, I want to see a 2D node-link graph showing all my active supply chain routes, so that I can visualize resource flow in real-time.

**Acceptance Criteria:**
- The Unified Editor (`Spec 007`) provides a "Logistics View" window.
- Entities with an `Inventory` and active `LogisticsNetwork` (from Spec 025) are represented as nodes.
- Animated edges (lines) visualize the rate and volume of resource transfer between nodes.

### User Story 2 - Bottleneck Analysis Overlays
As an operator, I want to see which inventories are nearly empty or full at a glance across my entire lunar infrastructure.

**Acceptance Criteria:**
- The logistics view highlights nodes in red if they are "Resource Starved" or "Storage Full".
- Tooltips provide detailed telemetry on inventory levels without needing to teleport the 3D camera to the site.

### User Story 3 - Global Supply/Demand Dashboard
As a mission manager, I want a high-level dashboard summarizing my total production and consumption rates across all lunar assets.

**Acceptance Criteria:**
- A dashboard aggregates the production (e.g., d(Oxygen)/dt) and consumption (e.g., d(Power)/dt) metrics from all active `FactoryNode`s.
- Total "Stock on Hand" is visualized for the entire colony.
