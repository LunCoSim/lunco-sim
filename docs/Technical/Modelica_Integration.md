# Modelica Integration

LunCoSim supports high-fidelity physics modeling using **Modelica**, a non-proprietary, object-oriented, equation-based language for complex physical systems.

## Overview
The integration allows LunCoSim to solve Modelica-defined systems (e.g., electrical circuits, thermal networks) alongside its native game logic.

## Key Components

-   **Lexer & Parser**: Custom GDScript implementation (`core/modelica/lexer.gd`, `parser.gd`) to read Modelica syntax.
-   **Equation System**: Converts Modelica models into a system of differential-algebraic equations (DAEs).
-   **Solvers**:
    -   `RK4Solver`: Runge-Kutta 4th Order solver for continuous time simulation.
    -   `CausalSolver`: Resolves causality for interconnected components.

## Usage
Modelica files are typically placed in `apps/modelica/models/`. The simulation loads these files, parses the equations, and steps the solver each physics frame.

> [!NOTE]
> This feature is experimental. Not all Modelica 3.4 standard library features are supported.
