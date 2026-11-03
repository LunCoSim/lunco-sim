# Site frame without a local terrain surface

A geodetic site anchor owns a coordinate frame. It does not require a local
Plane or DEM when the scene uses the body surface directly. The handoff owner
now retires local handoffs when no local surface is authored. A declared flat
surface still uses its normal geometry contract; multiple owners retain the
structured flat-surface-cardinality diagnostic.

No synthetic ground, default plane or guessed terrain source is created.
Orbital imagery uses its authored global elevation and colour. Local contact
scenes continue to use their own declared terrain.

Production validation: the authored site_frame_without_local_terrain.rhai gate
passed all three checks in visible API session 4493, PID 2349809, using executable
SHA256 55f484012d9fe748899ae265f2dda7f476287e541d0f11367aa7be9494462751.
It accepts no local terrain, authors two valid competing flat-site surfaces,
requires the exact diagnostic, removes them and requires successful recovery.
Temporary test edits were discarded without saving the scene file.
Native log retained in the Griffin V13 R4 evidence/site-frame-native-gate.log.

Malformed authored flat-site geometry now raises the terminal scene fault
`usd-local-surface-invalid` as well as the telemetry/status event. A warning
alone did not make GetReadiness fail. Native validation in PID 2881370,
executable 00ea8e56241fcf0fd110faf226307bac5e2028068993589ab8a53ec385da1d29,
authored a temporary axis=Z Plane, observed faulted readiness with the exact
prim path and required axis=Y rule while the application/API stayed available,
and recovered by discarding the test edit, awaiting its completion, closing the
retired source document and reopening the correct source. No malformed edit
was saved. Evidence: local-surface-error-native-r4.json in the film evidence.

The user approved local work without Trello. This change remains local on
optimization; it has not been pushed.
