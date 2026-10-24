# Native celestial evaluation and checked civil time

The application pins Celestial core, ephemeris and time to upstream
`609f3fa64d068fd890d713785113803f62a07b94`. No dependency source was modified.
`lunco-celestial-ephemeris` owns the analytic position adapter;
`lunco-time` owns civil-time conversion, and its scene-time policy/UI are the
consumers of checked results.

Native position-only evaluation skips unused rate terms and uses the library's
vectorized trigonometry. All native body positions, including the geocentric
Moon, use ICRS AU. The provider converts at its existing typed ecliptic boundary
and retains its exact-epoch result/operand cache. The native Earth evaluator owns
Earth/EMB mass-ratio composition. A changed native numerical fingerprint is
expected; cross-version bit equivalence is not an acceptance claim.

The previous landed native Tracy capture shows one expensive Moon evaluation
per physical tick, rather than duplicate evaluations at the same boundary.
The exact-epoch cache remains sufficient for that observed consumer. The
256-epoch diagnostic measures the complete provider at 149.799 ms before the
dependency update and 109.018 ms after, approximately 27% less elapsed time.
That isolated measurement is not the complete coupled tick.

UTC parsing, leap-second validation, conversion and millisecond ISO formatting
use native Celestial. The portable wall clock supplies Unix seconds/nanoseconds.
Conversion errors remain errors. Scene-time facts supply either the computer
candidate or its error; a valid authored epoch does not require computer time.
The required Rhai policy chooses, the time owner validates the selected
candidate, and invalid selection keeps scene admission held. The sky clock
displays unavailable UTC and warns on rejected conversion actions.

Verification uses regular `target/` builds with `-j 4` and sccache:

- Application compile check and production build pass. The pre-existing
  `UsdStageProjectionPlan` unused import warning remains outside this change.
- Three time-owner tests pass: atomic offset/ISO round trip, fractional and
  genuine leap seconds, and rejected dates/nonfinite epochs.
- Four ephemeris-owner tests pass, including exact-epoch cache invalidation,
  polar Sun geometry and lunar orientation across epochs. The manual benchmark
  stays ignored in ordinary tests.
- Production hook policies pass 46 checks, including both time candidates and
  unavailable computer time. Solar visibility passes eight checks; night passes
  two; invalid solar admission reports its expected terminal held fault.
- Native desktop input verifies an invalid UTC seek emits a warning and leaves
  the paused celestial epoch unchanged, a valid UTC seek applies the checked
  atomic offset, and Now agrees with the computer time within two seconds.
  Inspected clock-menu captures show the native ISO values. Wayland rejects
  cursor-warp requests during this synthetic-input run; the menu interactions
  and epoch assertions pass, without a renderer panic or validation fault.
- Initial velocity passes five checks. Contact containment passes ten. Griffin
  high-impact containment passes seven: a 299.677 m/s contact pauses all 20
  bodies and the continuous observer/controller/guidance clocks with one warning.
  Nominal landing passes five checks with no false pause and continued model time.
  These gates qualify model-validity containment, not survivable impact absorption.

The physics gates use binary SHA-256
`feca4ae915663962383a62bef8a7cc2320c3945a0545227b71e2304ec91614c7`.
The subsequent UI-only status-button repair is independently qualified in
[the suspension review](coupled-simulation-suspension.md). Three High-quality
desktop observations and solar gates use
`fbe1161b180a140aa85c5b99d94cc139264ed76dfcb2372f695e283c60d7115b`:

| Twin | OpenTwin to ready (s) | Complete tick median/p95 (ms) | Frame median/p95 (ms) |
|---|---:|---:|---:|
| Summer | 3.083 | 3.343 / 3.729 | 4.715 / 10.053 |
| Griffin | 27.705 | 5.682 / 6.761 | 12.980 / 16.538 |
| Tesla | 2.638 | 1.640 / 2.027 | 3.988 / 6.389 |

Each observation requires five seconds of stable readiness and full terrain
admission, then a ten-second window with unchanged authored source hashes,
clear diagnostics/script errors, and an inspected final image. Another
worktree's Tesla process on port 4387 remains a concurrent workload. These
startup/landing windows are not an exact replay comparison or full mission
acceptance. Griffin reaches touchdown at tick 1339 and handoff at 1340, with no
repeated compiler model identity in that run. The reported mid-flight reload
remains open pending its affected model/trigger. Tesla's frame/datum HUD remains
unavailable in the inspected capture; it is not qualified by this change.

Receipts, logs, source hashes and images stay ignored under
`.cache/codex-evidence/performance-20261023/`: `celestial-upstream-*`,
`*-celestial-upstream-normal/`, `celestial-clock-ui/`, and
`instability-latest/celestial-*`.
The under-one-second loading, under-one-millisecond coupled tick and
under-ten-millisecond frame targets remain open. A separate post-update native
Tracy attribution capture is still required before claiming an integrated
CPU-owner improvement.
