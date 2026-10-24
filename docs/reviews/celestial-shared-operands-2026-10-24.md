# Shared native Earth/Moon operands

The application pins published Celestial commit
`3ceda3bbc8770023e0ab527e826828b7e9bf5aec` on
`LunCoSim/celestial` branch `fix/shared-earth-moon-positions`, based on maintained
upstream `609f3fa64d068fd890d713785113803f62a07b94`. The remote SHA matches and
the fork is clean. The user explicitly approved modifying and publishing it.

Native `Vsop2013Earth::heliocentric_position_from_samples` composes Earth's
position from checked VSOP2013 EMB and ELP/MPP02 Moon samples. The ordinary
native Earth evaluator uses the same composition owner. The application's
existing exact-epoch cache retains the geocentric Moon operand alongside its
heliocentric operands, eliminating repeated native theory evaluations inside
Earth's calculation. The independent application Earth evaluator is removed.
No Earth/Moon formula is copied into application code.

Native sample acquisition owns validity checks. The provider's serialized cache
ensures the samples have one exact epoch; a different Julian-Date bit pattern
invalidates final and intermediate samples, including failed evaluations.
Native ICRS/AU f64 values, coefficient sets, arithmetic/rotation order and the
parent tree are retained. There is no approximation, time quantization or
cadence change. EMB-only queries retain their independent valid epoch range.

In the baseline settled Griffin capture, 240 complete Moon placement zones coincide
with the selected 240 completed FixedMain instances. Their median/p95 is
0.878/1.331 ms, with 223.896 ms total. These zones run on Bevy workers, not
the main thread; main-thread-only FixedMain CPU samples do not attribute them.
The Moon placement zone includes the Earth's native calculation as well as
the explicit Moon evaluation. These are instrumented, contention-affected
times, not a product-speed or anticipated-gain claim.

The capture uses PID 1346320/API 5211, collector 1346288/8431, 1 kHz sampling,
and the regular Tracy build. Bevy executor-bookkeeping spans are suppressed
with `RUST_LOG=info,bevy_ecs::schedule::executor=warn`; system, schedule and
native physics spans remain. The adjacent native exporter succeeds. The
capture is 404,063,499 bytes: 17,712 blocks and 403,992,645 compressed payload
bytes. Main-thread FixedMain sample selection verifies 1,329 CPU samples,
240 matched zones and zero missing stacks. Its median is 8.912 ms; the
instrumented runtime receipt reports 8.840/12.563 ms median/p95 coupled service.

Griffin completes touchdown and four-pad handoff before five seconds of
readiness soak and a ten-second measured window. Terrain remains fully ready,
registered diagnostics are clear, source hashes remain unchanged, and the
inspected image retains the lander, terrain and shadow. The app and collector
exit normally and release their ports. Another Tesla session, PID 1346528/API
4499, consumes about 329 percent CPU. Loading, coupled-tick and graphics
acceptance targets remain open.

## Validation

The new native API regression fails to compile before the API exists. All 87
native ephemeris checks then pass, with three existing diagnostics ignored;
strict Clippy passes. Tests include exact position bits at validity boundaries,
non-finite inputs and finite-input arithmetic overflow. No native error is
replaced by a zero or fallback evaluation.

All eight application ephemeris checks pass, including the manual diagnostic,
exact native component bits across concurrent forward/reverse callers,
independent epoch limits, missing-result lifetime, adjacent epochs and the
existing Shackleton sunlight and lunar-facing frame checks. Owner-only Clippy
passes with warnings denied. The broader lint invocation stops at a pre-existing
`collapsible_if` in `lunco-runtime-context`; no suppression or unrelated source
change is introduced. The lockfile changes only the three Celestial source pins.

Five alternating pairs of the exact application consumer diagnostic measure
109.926 ms baseline versus 54.539 ms candidate for 256 unique epochs, a 50.4
percent reduction. Every run retains fingerprint
`[13311947988806263165, 8917628640260778995, 10498597898422901494]`.
This is a complete-provider diagnostic, not a whole-tick or loading gain.

The regular production build passes in 2m22s, SHA-256
`0d494061add2277a7934c768b9f76bbe0d742d3110014103df1fe0047556246b`.
Production terrain solar visibility passes eight authored checks on API 5212,
night passes two on 5213, and invalid admission passes its expected terminal
`solar-sample-unavailable` verdict on 5214. All three exit zero and release their
ports. The coordinator initially omitted `terminal` when matching the negative
verdict text; the original exact terminal PASS and zero exit are rechecked
without repeating the runtime test.

Normal Griffin PID 1423639/API 5215 completes touchdown and four-pad handoff,
five seconds of complete readiness and a ten-second window with at least 240
fresh coupled ticks. Sources remain unchanged, diagnostics are clear and the
lander/terrain/shadow image is inspected. The app exits normally. Source
revision remains `78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`. First requested-Twin
physics readiness is 18.309 s; landed eligibility is 53.192 s. Coupled service
median/p95 is 4.492/8.688 ms and frames 27.481/41.549 ms. Concurrent tutorials
compilation, a Tesla session and video encoding qualify these observations.
The initializer still spends 2.49 s compiling, 1.013 s loading its prepared graph
and 3.557 s constructing its session, with 16,414 hits in 16,482 native attempts.

The matching filtered Tracy run uses PID 1508079/API 5216 and collector
1508071/8432. Source hashes match the baseline exactly, landing/readiness and
the inspected image pass, and both processes close. Native export succeeds.
Its 374,647,634 bytes contain 17,176 blocks and 374,578,924 payload bytes. Native
sampling verifies 1,000,000 ns, 240 completed FixedMain zones, 659 main-thread
CPU samples and zero missing stacks. The selected Moon placement zones have
0.263/0.364 ms median/p95 and 66.791 ms total. The observed full FixedMain median
is 4.248 ms. Different host pressure prevents treating the baseline comparison
as an isolated whole-runtime speedup; the alternating consumer diagnostic is
the bounded gain evidence.

The final regular build passes in 42.12 s, SHA-256
`5f35c5dd285532f66f1dcfbf1b064d92d8f7a00c69dbf44e8b0b3ee5e993d135`.
Additional normal Summer/API 5217 and Tesla/API 5218 windows require five
seconds of full terrain readiness and ten measured seconds with at least 240
fresh coupled ticks. Sources stay unchanged during each run, registered
diagnostics are clear, inspected images retain scene output, and both apps close.

| Twin | First requested scene ready | Coupled median/p95 | Frame median/p95 |
|---|---:|---:|---:|
| Summer | 2.085 s | 3.166/4.738 ms | 7.975/11.969 ms |
| Tesla | 2.885 s | 1.573/2.891 ms | 4.134/7.111 ms |

Summer retains source revision `9859e30efd0950a9735e9eb7feadee9aa5320b18`.
Tesla uses newer revision `949d49eff406d4ef82c964c10e981adb4adf461c` with
pre-existing authored edits; it is a different source cohort. Its existing
frame/datum-unavailable HUD remains visible. These initial ready windows are
not complete mission or clean performance acceptance. All original targets
remain open. The temporary baseline benchmark hard link is removed.

Raw evidence remains ignored under
`.cache/codex-evidence/performance-20261023/griffin-current-coupled-filtered/`
and `scripts/perf/captures/`, with owner checks and comparisons in
`celestial-*`. The earlier recovered capture has delayed
collector attachment and pressure/profiler overhead; it is not the timing
cohort for this change.
