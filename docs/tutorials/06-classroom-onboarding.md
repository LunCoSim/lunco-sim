# Classroom onboarding: Lunar Rover Lab

Lunar Rover Lab is the default first activity for a workshop participant.
Choose **Start Lunar Rover Lab** on the opening screen, or **Tutorials →
Start Here → Lunar Rover Lab**. The larger demo sandbox remains available
for free exploration.

The entry point opens the bundled classroom Twin through `OpenTwinAsset`.
Its manifest selects a document-backed scene and embeds the workshop guide.
The asset owner resolves the manifest on a worker; Workspace owns the normal
ordered scan and replacement commit. Edits remain in the open document until
the participant saves a personal Twin.

The practice yard uses bundled geometry, a single skid-steer rover, fixed
lighting, a marked lane, a gentle ramp, and an orange block beside a blue
authoring bay. It does not request a DEM, an ephemeris download, a Python
runtime, or a multiplayer connection. Install the normal application asset
bundle before class; those choices do not remove its fonts, shaders or
Modelica-library requirements.

## A 30-minute classroom segment

| Suggested time | Task | Completion evidence |
|---|---|---|
| 0–5 minutes | Take control, drive to the green marker, and stop | The intended rover is controlled, occupies the marker's Avian sensor, and moves below 0.15 m/s |
| 5–9 minutes | Open the speed graph and identify motion | The exact rover's retained `linear_speed` channel contains at least three samples and motion above 0.1 m/s |
| 9–16 minutes | Predict and run two drive-demand trials | Both three-second trials move the rover; total mass remains unchanged |
| 16–20 minutes | Explain the comparison | The chosen higher/lower/similar interpretation agrees with measured speeds |
| 20–25 minutes | Move the orange block into the blue bay | A persistent document translation is authored, with the block within 0.75 m of `(9, 0.5, 3)` metres |
| 25–30 minutes | Save a personal Twin | The document is saved and writable, and its exact admitted Twin owner reports a manifest; the saved report reopens |

These are teaching estimates, not measured usability results. Participants
advance by performing the task and using its check button. Waiting or opening
a file picker does not complete an exercise.

## Controls and recovery

The lesson displays the active semantic input bindings. **Take control**
acquires the training rover directly; selection and possession remain separate
operations. **Right rover** uses the generic `RecoverVessel` command to right
the vehicle and zero its velocities at the current location. It preserves
lesson progress. **Return camera** restores the authored overview, with rover
and destination in the same view. **More help** keeps actionable guidance on
screen. HUD buttons also provide a mouse path through control, assessment and
navigation; driving still uses the configured controller inputs.

The workshop card remains visible in View, Analyze, Build, Editor and Lunica.
Its named HUD has a larger text scale and bounded width; the existing HUD
context menu lets an instructor adjust those presentation settings.

**Instructor checkpoints** begins driving, telemetry, or comparison practice
in the current yard. A jump explicitly marks earlier tasks as unassessed;
that practice flag is retained in the saved report. A telemetry checkpoint
still requires real motion before its signal check can pass. Instructor
checkpoints preserve the yard. **Restart whole lab (discard edits)** explicitly
resets the document through the ordinary scene restart.

## What the experiment means

The baseline uses drive demand `0.25`; the changed run uses `0.65`. Both use
the same rover, heading, flat support, total mass, and a three-second drive
interval in simulation time. Recovery zeroes native body velocities, and a
one-second braked settling interval precedes each run. Trials take place at
successive positions on the same uniform flat lane. They do not claim an
identical world-coordinate start or a reset of battery and thermal history.

Rhai sequences these discrete operations. The authored Modelica controller,
motor models and native physics calculate motion. The recorded result is the
native linear-speed magnitude at the end of each drive interval. Speeds less
than `0.02 m/s` apart are classified as similar for this exercise. The initial
prediction need not be correct; the interpretation must match the evidence.

This is an illustrative simulation experiment, not a calibrated hardware
performance claim. Ask participants to identify additional controls needed
for a stronger experiment, such as battery state, temperature, repeated trials,
and surface variation. Mass and topology changes belong in subsequent
authoring exercises because they can require physical-scene readmission.

## The saved artifact

The template report has explicit zero-valued, unmeasured slots. A zero slot
cannot pass the save assessment. The completed report is an ordinary USD
program with `double` input values for measured
speeds, masses, the prediction code, and the practice flag. The report program
reads those authored values; it does not fabricate missing results. **Save as
Twin…** uses the existing `SaveAsTwin` document-promotion workflow and asks for
a new folder. Cancelling leaves the save step unfinished.

Reopening the saved Twin displays the recorded measurements and a next driving
exercise. The saved scene retains standard `lunco://` references to the
installed shared library. It is a project that uses the normal LunCoSim asset
bundle, not a self-contained export of every referenced asset.

## Tours, exercises, and continuation

The catalog distinguishes **tour**, **exercise**, and **challenge**, and shows
estimated minutes. A tour reviews interface guidance; it does not certify
that its suggested editing or engineering actions were performed. Skipping
emits `TUTORIAL_SKIPPED`, closes the tour, and cannot later become completion
through stray Next events.

The application asset policy resolves `next_source_asset` links into one
typed learning path. Missing targets and cycles reject the catalog visibly.
The shared `tutorials` tool supplies the continuation buttons and cursor
policy. A standalone scenario without catalog context promises no next button.
Lesson-specific objectives and classroom decisions stay in Rhai.

## Verification and classroom pilot

Eleven behavioral checks live in
`assets/scenarios/tests/tutorial_rover_lab.rhai`, composed by
`assets/scenes/tests/tutorial_rover_lab.usda`. It exercises early and wrong
actions, sensor identity, live driving, recorded motion, prediction gating,
two real trials, interpretation, rejection of runtime-only placement, authored
placement, and unsaved-report state.
Use the production binary:

```bash
"$LUNCOSIM_BIN" test --scene scenes/tests/tutorial_rover_lab.usda --max-ticks 6000 --api 4198
```

The windowed acceptance driver additionally checks the opening screen,
rendered lesson/graph, Twin promotion, saved report and block placement after
reopening, tour skipping, instructor checkpoints and document reset:

```bash
LUNCOSIM_BIN=target/debug/luncosim python3 scripts/api/test_classroom_onboarding.py --port 4197
```

Inspect its screenshots at ordinary classroom resolution. The driver uses the
production command and lesson-action surfaces. API and headless verdicts do not establish classroom
usability, projector readability, package completeness or offline installation.

Before using the flow as a default curriculum, pilot it with 5–8 first-time
participants. Record time to first successful drive, help requests, recovery
time, successful saves and whether each participant can explain the comparison.
Aim for a first drive within three minutes and recovery without instructor
intervention; treat these as proposed acceptance targets until measured.
