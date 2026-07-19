# Engagement mechanics — research catalog

Research snapshot, 2026-07-19. What makes rover/space sims engaging, from real
ops history, competition rubrics, teleoperation literature, and comparable
games — mapped onto the substrate this engine already has. Inspiration/planning
material, not a description of running code.

## The thesis

The engine's depth (real DEM, LOS radio, Modelica power, prediction netcode,
teach mode) is largely invisible or consequence-free to a player. Every
mechanic below works by making something *already computed* load-bearing for
the player. Almost nothing here needs a new subsystem.

## Signature feature: Lunokhod crew stations

Lunokhod 1 was driven from a bunker near Simferopol by a five-man military
crew, on single still frames every 7–20 s (Lunokhod 2: ~3 s) across a ~2.5–3 s
round-trip delay, in two-hour shifts with doctors logging elevated heart rates.
The role split maps directly onto multiplayer + panels + asymmetric information
(the Artemis: Spaceship Bridge Simulator pattern — no station can succeed
alone, so communication becomes the gameplay):

| Role | Historical duty | Console (all data exists today) |
|---|---|---|
| Commander | go/no-go, arbitration, timeline | objectives + score + crew status, **no controls** |
| Driver | joystick from still frames across delay | camera + navball only, **no map** |
| Navigator | position from gyros + 9th odometer wheel | map/DEM (fog-of-war), waypoints, headings |
| Flight engineer | motor currents, tilt, temps, battery | Modelica telemetry ports, calls aborts |
| Antenna operator | high-gain pointing at Earth | link margin, comm-shadow prediction (link kernel) |

Scales: 2 = driver+navigator · 3 = +engineer · 4 = +comms · 5 = +commander or
science officer (Basilevsky historically backseat-drove: "Stop, show me that!").
Instructor = Mission Director via teach mode, injecting faults and time
pressure. Built-in cautionary tale: Lunokhod 2 died of haste (rushed crew,
crater wall, dust on the radiator) — "haste kills rovers" is a historically
grounded scoring philosophy: penalise interventions and tilt excursions more
than slow time.

## The difficulty system: mode multipliers (ERC/URC-derived)

ERC/URC score the *mode of driving*, not just the outcome — ERC teams losing
autonomy dropped ~80% of achievable points. One course, four ways to drive it:

| Mode | Multiplier | What it uses |
|---|---|---|
| Live video teleop | ×1.0 | today's driving |
| Delayed (2.6 s RTLT) | ×1.5 | `light_time_s` — already computed per peer |
| **Lunokhod mode** (delay + still frames + instruments only) | ×2.0 | frame-cadence render throttle; cheapest distinctive feature in the catalog |
| Scripted/autonomous (rhai plan, no intervention) | ×3.0 | autopilot + rhai — the in-app rhai editor becomes *content* |

Adapted 100-pt rubric: checkpoints 60 (≤3 m full / ≤5 m half, URC tolerances) ·
final-stop precision 10 · **pre-brief/route plan submitted 10** (URC scores the
review — this scores the QGIS team explicitly) · time bonus 10 · science stop
10 (stillness + tilt gate). Penalties: tip-over −20 and run ends · intervention
−10 · tilt excursion −5 · blackout >30 s −5 · corridor deviation −2.

## Signal delay: the literature's verdict

Raw delay on live video + joystick is the *frustrating* configuration
(Sheridan & Ferrell: beyond ~1 s operators adopt move-and-wait; completion time
≈ moves × delay). Structured delay is a great mechanic. Affordances that make
it playable, in order:

1. **Frame-cadence camera** — turns latency into a decide-per-frame rhythm
   (historically authentic, and each frame is a shared event the crew debates).
2. **Predictive ghost rover** (JPL "phantom robot" line) — an undelayed local
   simulation overlay; the prediction netcode already dead-reckons this.
3. **Command-in-flight timeline** — HUD strip showing commands going up and
   imagery coming down, so players blame physics, not the game.
4. **Stopping-distance marker** — "if you command STOP now, you halt HERE."
5. **Waypoint queueing with rover-side hazard auto-stop** — supervisory
   control, which is MER practice and ESA METERON's conclusion.

## Top mechanics by engagement-per-cost

1. **Coopetition** — cooperate inside the team, leaderboard between teams
   (FIRST alliance model; best-evidenced format in STEM education). Zero code.
2. **Timed checkpoint traverse + mode multipliers** (above). Rhai only.
3. **Lunokhod frame-cadence mode.** Render throttle + UI.
4. **Crew stations.** Multiplayer + panels exist; needs per-station panel sets.
5. **Radio LOS as a terrain puzzle** — VIPER never drove out of Earth LOS;
   blackout zones as plannable, predictable-by-viewshed spatial constraints.
6. **Battery/illumination windows** — VIPER's sortie-into-shadow loop; a
   visible SoC budget vs a moving terminator is a countdown without a timer.
   Needs the one missing cosim producer (irradiance from sun geometry).
7. **Terrain-reading risk/reward** — SnowRunner/Death Stranding: route choice
   IS the game when terrain is the antagonist; score slip/tilt events so the
   safe-long vs risky-short tradeoff is priced.
8. **Blind-plan sol cycle** — MER tactical loop (downlink → plan → uplink →
   hands-off execution) as a turn-based mode; the QGIS team *are* the planners.
9. **Spectator mission-control wall** — waiting teams watch live map + downlink
   + telemetry + score; waiting becomes scouting.
10. **Failure-as-content** — KSP's lesson: honest failure teaches. A tipped
    rover produces a 30 s replay for the room and *stays on the map* as the
    next team's rescue objective (Opportunity at Purgatory Dune).
11. **Role rotation per round** — the Soviets rotated crews for stress; FIRST
    rotates for engagement. Zero code.
12. **Fog-of-war on the DEM** — reveal high-res truth only where sensors
    looked; gives the remote-sensing team a live role *during* the drive.
13. **Instrument/sampling with stillness gates** — Take On Mars' cautionary
    lesson: placement gameplay dies under jitter, so gate on "stationary, tilt
    < X, settled N s" (which is also how real rovers work).
14. **Navball-style tilt instrument with published limits** — KSP: trust the
    instrument over the view; the per-rover HUD arcs already exist.
15. **Predictive ghost** (above) — ship as "modern ops" counterpart to
    Lunokhod mode; same course, 1970 tools vs 2030 tools, compare scores.

## Ten mission archetypes (one-line precedent each)

1. Timed checkpoint traverse — ERC Navigation / URC Autonomous Navigation.
2. Blind-plan sol cycle — MER/MSL tactical ops.
3. PSR sortie / battery window — VIPER ops concept.
4. Site survey with fog-of-war — photogrammetric survey; cartography loops.
5. Sample-return chain — Perseverance depot; ERC Probing task.
6. Rover rescue — Opportunity's Purgatory Dune; SnowRunner recovery.
7. Convoy/cargo logistics — Death Stranding load management; Artemis LTV.
8. Comm-relay deployment — farside/PSR relay concepts; native to the link kernel.
9. Night/eclipse survival — Moonbase Alpha's 25-minute scenario; Chang'e night.
10. ISRU-lite construction — ERC Maintenance; Artemis base-camp buildup.
