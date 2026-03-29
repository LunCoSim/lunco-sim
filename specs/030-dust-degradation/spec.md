# Feature Specification: 030-dust-degradation

**Feature Branch**: `030-dust-degradation`
**Created**: 2026-03-29
**Status**: Draft (Upcoming Plugin)
**Input**: Regolith accumulation, thermal blanket degradation, and mechanical joint abrasion.

## Problem Statement
Apollo missions demonstrated that lunar dust (regolith) is the most pervasive and destructive environmental hazard on the Moon. It degrades solar panel efficiency, fouls thermal radiators by changing surface emissivity, and acts as an abrasive in mechanical joints. To accurately model long-term lunar settlement operations, the simulation must track dust accumulation natively as an environmental state that impacts vehicle physics and thermodynamics.

> **Note on Scope**: This is an upcoming plugin. The foundational terramechanics (`027`) must exist first to source the dust.

## User Stories

### Story 1: Solar Array & Radiator Fouling
As a power systems engineer, I want solar panels and radiators to progressively lose efficiency if they operate near dust-kicking activities (like landing engines or roving).

**Acceptance Criteria:**
- An `EnvironmentalDegradation` component tracks the fractional accumulation of dust on exposed surfaces.
- Dust coverage explicitly degrades the electrical output of solar arrays.
- Dust coverage modifies the emissivity value passed to the Modelica thermal solver (`014`), leading to progressive overheating.

### Story 2: Mechanical Abrasion
As a robotics engineer, I want the joints of my excavation rover to require more power over time as dust enters the mechanisms.

**Acceptance Criteria:**
- High-dust environments increase a `FrictionMultiplier` on actuator joints in the Plant layer.
- The `OBC` layer will register higher current draw to achieve the same physical torque.

### Story 3: Visual Dust Wear
As an optics engineer, I want camera lenses and rover chassis to visually reflect dust accumulation.

**Acceptance Criteria:**
- A custom shader layer maps the internal dust accumulation variable to an overlay texture, physically dirtying the vehicle in real-time.
- "Wiper" mechanisms or cleaning events can instantly reset the dust accumulation variable to baseline.

## Implementation Notes
- Rather than simulating billions of individual particles, dust is treated as a localized field/volume density triggered by actions (driving, landing). Entities within the field accumulate a scalar `wear` value over time.
- Directly feeds into existing `PowerBus` and `Modelica` ECS payloads.
