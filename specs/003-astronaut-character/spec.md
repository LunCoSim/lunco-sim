# Feature Specification: Simple Astronaut Character

**Feature Branch**: `003-astronaut-character`
**Created**: 2026-03-31
**Status**: Draft
**Input**: User description: "we need a simple astronaut character. It must be lightweight and generated. We should use kinematics + maybe lightweght neuralnets with basic bones for animating astronaut. It's ok to prepender some of the animations once downloaded. We will need basic movements and jumps at minimum. Idually we also need crawls, and interactions with doors, driving rover and maybe something more like interaction with the environment"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Basic Locomotion (Priority: P1)

As a player, I want to control an astronaut character that can perform basic movements like walking and jumping, so I can explore the environment on foot.

**Why this priority**: This is the most fundamental interaction with the character and the primary way for a player to engage with the world at a human scale.

**Independent Test**: The player can spawn into a test scene and control the astronaut character, making it walk around and jump onto a small obstacle.

**Acceptance Scenarios**:

1.  **Given** an astronaut character is spawned in the simulation, **When** the player provides forward input, **Then** the character plays a walking animation and moves forward.
2.  **Given** the astronaut character is on the ground, **When** the player provides a jump input, **Then** the character plays a jump animation and moves vertically, affected by the current gravity (e.g., lunar gravity).

### User Story 2 - Procedural Animation & Kinematics (Priority: P1)

As a developer, I want the astronaut's animations to be driven by a lightweight, kinematics-based system rather than pre-baked animation files, so that the character can react dynamically to the environment.

**Why this priority**: This fulfills the "lightweight and generated" requirement and provides a flexible foundation for more advanced interactions. It reduces asset size and allows for more believable physics-based movements.

**Independent Test**: In a test scene with uneven terrain, the astronaut's feet will correctly plant on the ground at different heights (Inverse Kinematics), and its body will have a subtle, physics-driven procedural sway.

**Acceptance Scenarios**:

1.  **Given** the character model has a defined skeletal structure (basic bones), **When** the character walks over a slope or small rocks, **Then** an Inverse Kinematics (IK) solver adjusts the leg and foot positions to plant them realistically on the surface.
2.  **Given** the character is standing still, **When** pushed by a small physical force, **Then** a procedural animation layer or lightweight neural network generates a balancing motion, rather than playing a static "hit reaction" clip.

### User Story 3 - Advanced Environmental Interaction (Priority: P2)

As a player, I want the astronaut to be able to perform complex interactions like crawling under obstacles, opening doors, and driving rovers, so I can engage in more meaningful missions.

**Why this priority**: These interactions elevate the character from a simple avatar to a functional agent capable of completing complex tasks.

**Independent Test**: The player can approach a low-hanging pipe, press a "crawl" button to move under it, approach a door panel and open it, and enter and take control of a rover.

**Acceptance Scenarios**:

1.  **Given** the character is facing a low obstacle, **When** the player activates "crawl" mode, **Then** the character's posture changes, and they can move forward in a crawled state.
2.  **Given** the character is near a door control panel, **When** the player initiates an "interact" action, **Then** the character's IK system reaches a hand out to the panel, triggering the door to open.
3.  **Given** the character is near the driver's seat of a rover, **When** the player initiates an "enter vehicle" action, **Then** the character model attaches to the seat, and player controls are transferred to the rover (as defined in spec `001-vessel-control-architecture`).

## Requirements *(mandatory)*

### Functional Requirements

-   **FR-001**: The system MUST provide a simple, low-polygon astronaut character model with a basic skeleton (armature).
-   **FR-002**: Character locomotion (walking, jumping) MUST be implemented.
-   **FR-003**: Animations MUST be primarily driven by a procedural system using kinematics (e.g., Inverse Kinematics for foot placement).
-   **FR-004**: The system SHOULD explore lightweight neural networks (e.g., Motion Matching alternatives) for generating fluid, dynamic movements as a potential enhancement to pure IK.
-   **FR-005**: The character MUST have a state machine for managing different behaviors (e.g., walking, jumping, crawling, interacting).
-   **FR-006**: The character MUST be able to interact with specific objects in the environment, triggering events.

### Key Entities

-   **AstronautCharacter**: The main Bevy entity containing the model, skeleton, physics collider, and a controller for handling player input and state.
-   **KinematicAnimator**: A component that procedurally drives the character's bone structure based on its movement and interaction with the physics world.
-   **InteractionTarget**: A component that can be placed on objects like door panels or rover seats to define a point for the astronaut to interact with.

## Success Criteria *(mandatory)*

### Measurable Outcomes

-   **SC-001**: The total memory footprint for the character model and its animation system (excluding pre-calculated NN data) is under 10MB.
-   **SC-002**: The character can successfully navigate a test environment with varied terrain (slopes, small obstacles) without feet clipping or floating noticeably.
-   **SC-003**: The CPU cost of the procedural animation for one character is less than 2ms per frame on a mid-range CPU.

## Assumptions

-   A physics engine capable of handling character controllers is integrated into the simulation.
-   The "lightweight neural net" is not a hard requirement for the MVP, but a direction for future enhancement. The primary implementation will focus on kinematics.
-   Interaction points (like door panels) will be clearly defined and tagged in the scene for the character's systems to discover.
-   Rover driving mechanics are handled by other systems; this spec is only concerned with the character's ability to initiate the "driving" state.
