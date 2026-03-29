# Feature Specification: 026-native-chemistry-simulation

**Feature Branch**: `026-native-chemistry-simulation`
**Created**: 2026-03-29
**Status**: Long-Term Roadmap

## Problem Statement
To eventually support highly complex In-Situ Resource Utilization (ISRU), the engine must transition from merely calculating *rates of change* via simple logic, to actually hosting rigorous chemical kinematics and thermodynamic fluid simulations. To scale up without sacrificing the ECS `FixedUpdate` loop performance, we need to bind existing, vetted C/C++ libraries deeply into the Rust engine natively, circumventing Python orchestration overhead.\n\n> **Note on Scope:** Native Chemistry simulation is completely separated from the FMI boundary (`022`) and the internal Modelica math layer (`014`). This represents a distinct future iteration dedicated solely to high-performance C++ solver abstractions.

## Vision (Long-Term Implementation)

### 1. The `ChemicalSystem` Ecosystem
A `bevy_chemistry` plugin will be developed to bind directly to standard chemistry compute kernels (e.g., **Cantera** for complex equilibria and reaction kinematics, and **CoolProp** for pure fluid states). 
- Factories and habitats will be tagged with a `ChemicalSystem` component.
- During the `FixedUpdate` tick, C++ FFI bridges will natively ingest the pressure, temperature, and species mass-fractions from the Bevy ECS, solve the delta, and update the ECS asynchronously.

### 2. Engineering Generator Pipeline
By modeling accurate, data-driven chemistry natively within the game engine, LunCoSim transforms from a *consumer* of engineering models to an *author*. 
- Building a factory assembly in-game tests the chemical reactions dynamically.
- Upon success, the engine leverages the internal Cantera/CoolProp outputs to automatically serialize rigorous SysML item flows and Modelica thermal requirements `.mo` text formats for external engineering teams to utilize.

## Future User Stories

### Story 1: Native Reaction Solver
As an ISRU systems engineer, I want chemical cracking processes (Regolith -> Oxygen) to be calculated by a verified simulation kernel (like Cantera) natively in C++/Rust, avoiding any Python GIL-locking overhead during gameplay.

### Story 2: Procedural Modelica Output
As an engineering developer, I want to construct a factory using visual nodes, test its chemical yield in the ECS, and hit an "Export" button to automatically generate a formal `.mo` thermodynamics file outlining its precise power/heat profiles.
