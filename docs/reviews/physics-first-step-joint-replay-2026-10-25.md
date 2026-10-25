# First-step articulated replay investigation

Status: application integration of Rumoca `fbfbcdc9` is held. No reference or
zero tolerance is changed. This report records an unresolved failure, not a
confirmed Rumoca or Avian regression.

## Reproduction and established boundary

The regular pre-change artifact `786d8504` passes three targeted runs of
`jitter-0.5-seed-1234567890123456789`. The regular candidate `4ede2601` passes
eight full-matrix profiles, then fails this profile. The same candidate passes
an independent targeted repeat. A bounded admission-query diagnostic reproduces
a failure on attempt four; an existing bridge trace reproduces one on attempt
two; the extended native-state trace reproduces one on attempt three.

The maintained production Rhai verdict first reports an articulated mismatch at
tick 11. Later physics and Modelica rows also diverge. Direct native-state
traces locate the difference in the first completed physical step, before
contact. Before that step, recorded positions, rotations, full inverse-inertia
tensors, centers of mass, projection markers and native solver body indices
match between the passing and failing extended-state runs. The bridge's pose
writes also match. The affected rover subset changes across diagnostic runs.

The admission diagnostic observes different ordering across independent rover
groups. The observed within-rover order and normalized joint definitions match
in its passing/failing pair; this does not establish the actual order of every
native solver pass. Eight runs with an added read-only probe pass, which does
not resolve the intermittent failure. Two rejected diagnostic schedule variants
exit during schedule initialization and are excluded from numerical evidence.
All temporary application source probes are removed. The regular artifact
`4ede2601` is restored exactly before the bounded Griffin observation.

## Next native owner investigation

The pinned Avian `95d02b3` executes XPBD position and velocity passes through
`apply_xpbd_joint_pass` using mutable ECS query iteration. Constraint execution
order, prepared solver data and per-substep body state are not exposed by the
current application boundary. The next diagnostic belongs at that native
consumer, alongside the existing first-step production reproduction.

Proposed dependency scope: instrument the actual first-step joint passes and
compare source identities, solver data, inputs and execution order; then repair
only the confirmed owner of the first-step variability. Preserve the f64 public
state, current solver precision, eight substeps, numerical checks, tolerances,
geometry and physical equations. Reuse native graph/lifecycle mechanisms for
any required ordering or invalidation repair, without copying a solver into
application Rust. Add the smallest native regression for the confirmed failure,
run dimension/precision checks as applicable, then the unchanged production
replay matrix and bounded normal Twin windows. Publish only a tested fix to the
existing LunCoSim Avian fork and pin its verified commit.

This extends the stationary child-pose change-detection approval. Native source
instrumentation, a fix and publication require explicit user approval under
AGENTS.md. No dependency source in this additional scope is modified yet.

Raw logs remain ignored in `.cache/codex-evidence/performance-20261023/` under
`rumoca-refresh-*`; profile data is not tracked. Production tests use the regular
checkout build, sccache and four Cargo jobs, with owned API port 5345. All test
processes close; the foreign session on port 4499 is untouched.
