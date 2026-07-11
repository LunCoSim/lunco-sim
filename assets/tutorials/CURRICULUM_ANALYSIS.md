# Tutorial Curriculum — Analysis & Plan

**Prepared:** 2026-07-11 · **Worktree:** `tutorials` · **Companion:** `~/Documents/models/summer_space_school/` (IKI event brief)

Two questions answered here: (1) what "proper" tutorials should cover — **sim features** *and* **UI
tours of new features**; (2) what the **IKI Summer Space School** (~25 Jul) actually needs, so the
same tutorials serve the event *and* general onboarding.

---

## 1. The event, in one paragraph

IKI runs a **2-hour seminar (training)** then a **scored simulation (practice)** — same skills, new
inputs, 2–3 variants simple→hard, points-scored. ~25 laptops, 10–11 teams, 1–2 drivers each. A **ДЗЗ
(remote-sensing/GIS) section** analyses the terrain and hands the rover teams a **route**; the teams
**validate it against rover limits and drive it**. Scene: 1000×1000 m, a rille (or crater) 40–50 m
deep with borderline slopes, an SE highland casting shadow, low ESE sun, Earth low on the E horizon so
the floor sits in **radio-shadow** (autonomy only). Hazards to teach: **slope/tip-over, sun-shadow,
radio-shadow, battery/thermal (bonus)**.

**The seminar *is* a set of tutorials.** That is the whole reason to get the tutorial curriculum right
now: the 2h training block is delivered as guided LuncoSim lessons.

---

## 2. Current coverage (8 lessons, `sandbox/tutorials.json` + scene-backed)

| id | kind | teaches | event-relevant? |
|---|---|---|---|
| `sandbox-intro` | coach tour | workspace: viewport/browser/inspector/console | ✅ general orientation |
| `first-drive` | mission | possess + drive a rover to a waypoint | ✅✅ core driving skill |
| `lander-mission` | mission | watch a descent, then drive deployed rover | ➖ nice, not core |
| `build-scene` | coach tour | palette + gizmo + inspector + USD | ✅ general authoring |
| `build-base` | mission | lay out a base from the Structures kit (siting rules) | ➖ general, not event |
| `script-a-rover` | coach tour | what a rhai scenario is; hooks, prelude verbs | ✅ instructor-side |
| `inspect-sim` | coach tour | live state: selection, ports, plots, API readback | ✅✅ telemetry skill |
| `cosim` | coach tour | Modelica model flies a physics body | ➖ bonus (battery/thermal) |

**Verdict:** good general-onboarding spine; **the event's specific skills are largely uncovered.**
Nothing yet teaches slope/tip-over, reading shadow & radio-shadow, following an imported route, rover
variants, or the scored run. `first-drive` and `inspect-sim` are the two that transfer directly.

---

## 3. Gaps — two categories the user named

### A. Sim-feature tutorials (the seminar skills)

| # | Proposed lesson | Teaches (task ref) | Build-readiness |
|---|---|---|---|
| S1 | **Drive on the Moon** | throttle/steer/brake, camera, wheel slip climbing a slope (§5) | **READY** — extends `first_drive`; needs a sloped mini-scene |
| S2 | **Read the Terrain** | slope, sun-shadow, radio-shadow zones — interpret *before* driving (§3.3) | **READY-ish** — visual/coach tour now; live rasters are a P1 build item |
| S3 | **Follow the Route** | imported ДЗЗ waypoints, reach POI + return, "validate route vs rover" (§3.3) | **PARTIAL** — trigger-zone waypoints exist; route import is P0.3 |
| S4 | **Rover Variants & Tip-Over** | CoM/torque/grip; why the "awful" rover tips; righting (§5) | **PARTIAL** — variants are USD-only (READY); tip-over detector is P1.5 |
| S5 | **Radio-Shadow → Autonomy** | comms drop on the floor forces autopilot; plan the traverse (§3.2) | **BLOCKED** on P1.4 zone + `piloted` gate wiring |
| S6 | **The Scored Run** | objectives, timer, penalties, scoreboard; the practice mission, 3 tiers (§1.2) | **BLOCKED** on P1.5 scoring rhai layer |
| S7 | **Battery & Thermal** (bonus) | energy budget, cool-in-shadow / overheat (§5 bonus) | **BLOCKED** on P2 modelica wiring |
| S8 | **Join a Team Session** | connect, possess your team's rover, instructor topology (§2) | **READY** — possession + per-team sessions exist |

### B. UI tours / new-feature intros (landed since Bevy 0.19, no tour yet)

| # | Proposed tour | New feature | Readiness |
|---|---|---|---|
| U1 | **Terrain Tools** | crater brush, dig/flatten, band-limited edit scaling | READY (feature shipped) |
| U2 | **Object Builder** | canvas + USD mounting/snapping UI | READY |
| U3 | **Rhai Editor & REPL** | in-app script panel + live REPL + USD param hints | READY (`rhai_editor`, `rhai_repl` panels) |
| U4 | **Save & Share** | SaveScenario (live rhai → USD attrs), share links | READY |
| U5 | **Inspector & Plots deep-dive** | telemetry ports → plots, API readback | READY (fold into/extend `inspect-sim`) |

### C. Enabling gap that blocks *all* good UI tours

Tutorial spotlighting currently anchors to **one generic region** (`panel.bottom` is the only anchor
used anywhere; the anchor registry in `lunco-tutorial/src/lib.rs` is a single centered rect). Proper
UI tours ("here is the Terrain Tools tab", "this gizmo handle rotates") need a **named-anchor
vocabulary** per panel/widget (`panel.palette`, `panel.inspector`, `panel.terrain_tools`,
`panel.rhai_editor`, `hud.objectives`, `viewport.gizmo`). This is a **small enabling task** and is the
highest-leverage thing to build first — every U-tour and half the S-lessons improve once it exists.

---

## 4. How the two audiences share one curriculum

```
GENERAL ONBOARDING (exists, keep):        SPACE-SCHOOL SEMINAR (new track):
  sandbox-intro                             S1 Drive on the Moon      ← reuses first-drive
    → first-drive  ───────────────────────▶ S2 Read the Terrain
    → build-scene → build-base              S3 Follow the Route
    → script-a-rover                        S4 Rover Variants & Tip-Over
    → inspect-sim  ───────────────────────▶ S5 Radio-Shadow → Autonomy
    → cosim  ─────────────────────────────▶ S6 The Scored Run  (the graded practice)
                                            S7 Battery & Thermal (bonus)
                                            S8 Join a Team Session
UI TOURS (cross-cutting, general + event):
  U1 Terrain Tools · U2 Object Builder · U3 Rhai Editor/REPL · U4 Save&Share · U5 Inspector/Plots
```

The seminar track is a **new `tutorials/school/` app manifest** (or a `first_start`-chained sub-path
in sandbox) so instructors launch it as one flow, while the general spine stays the default first-run.

---

## 5. Recommended build order

1. **U0 — anchor vocabulary** (enabling, §3C). Small Rust/UI: register named anchors for the standard
   panels + a couple of viewport/HUD targets. Unblocks every tour.
2. **Buildable now, no engine deps:** S1 Drive-on-Moon (sloped scene), S8 Join-a-Team, U1–U5 tours,
   S4a the *variants* half (author 3 rover `.usda`: easy/medium/awful). These land the seminar's
   backbone and all the new-feature intros without waiting on the roadmap.
3. **Track the P0/P1 roadmap items** (`04_IMPLEMENTATION_PLAN.md`): S3 needs route import (P0.3);
   S4b tip-over + S6 scoring need the rhai scoring layer (P1.5); S5 needs the radio-shadow zone
   (P1.4). Write these lessons *against the interfaces* now so they drop in when the build lands.
4. **Bonus last:** S7 battery/thermal after P2.

**Do-this-week for the 18 Jul demo:** U0 + S1 + S4a + S8 give a runnable seminar skeleton
(orient → drive → feel the variants → join a team) that needs zero roadmap items, and the tour set
(U1–U5) doubles as the "what's new" onboarding for general users.

---

## 6. Open decisions (for the user)
- **Seminar as its own app (`school/`) or a chained sub-path inside sandbox?** (Own app = clean
  instructor launch + reset; sub-path = less scaffolding.)
- **Language:** the IKI brief is in Russian. Author lesson copy **RU, EN, or bilingual**?
- **Which slice to build first** — the enabling anchors + buildable-now lessons, or wait and co-develop
  each seminar lesson with its roadmap build item?
