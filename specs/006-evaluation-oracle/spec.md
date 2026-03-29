# Feature Specification: 005-evaluation-oracle

**Feature Branch**: `005-evaluation-oracle`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Integrated TDD Verification Node for Headless CI/CD testing.

## Problem Statement
Thousands of scenarios (`004`) will be run in headless environments to validate autonomous driving algorithms and physics behavior. We need a definitive "Oracle" system that observes the simulation and decides when a scenario has officially "Passed" or "Failed" to enforce a Test-Driven Development (TDD) lifecycle without needing human visual review.

## User Scenarios

### User Story 1 - Victory Constraints (Priority: P1)
As a test engineer, I want to define a list of victory and failure rules alongside my scenario configuration, so that the simulation stops automatically when an endpoint is reached.

**Acceptance Criteria:**
- The engine implements an `Oracle` system evaluated during the `FixedUpdate` schedule.
- It interprets semantic rules parsed from the BSN/RON setup (e.g., `REQUIRE Rover.Transform.x > 100`, `FAIL_IF Rover.BatteryLevel < 0.05`, `FAIL_IF Time.simulated > 3600`).
- The Oracle triggers an `AppExit` the moment a definitive boundary is crossed.

### User Story 2 - Headless CI/CD Output (Priority: P1)
As an automated CI/CD pipeline, I test Pull Requests purely via CLI and need strict machine-readable output to pass or fail a GitHub Action.

**Acceptance Criteria:**
- Upon termination by the Oracle, the simulation writes a strict `test_report.json` to disk detailing the exact frame, simulation time, the rule that triggered the termination, and the final state of all requested `Sensors`.
- The engine application exits with standard POSIX exit codes (`0` for Pass, `1` for Fail, `2` for Timeout).
