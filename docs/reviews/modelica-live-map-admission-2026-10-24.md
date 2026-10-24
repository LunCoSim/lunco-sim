# Modelica live name indexes

`ModelicaModel` owns its reflected input and parameter maps. Both use Bevy's
maintained `HashMap`. The observable store uses native Bevy-reflected
`IndexMap`; its ordered-publication qualification is recorded in
[the focused review](modelica-ordered-observation-publication-2026-10-24.md). Response admission still
checks current input and parameter membership for every observation. No cached
classification, new state owner, missing-value default or dependency patch is
introduced. The worker reuses its name-sorted input sampler for both compile
admission and fixed-step dispatch. The USD input bridge uses native borrowed
entries and applies the authored command surface last, preserving precedence,
exact f64 bits and rejection of unrelated physical-sink names.

Constructors, the standalone Lunica entry, telemetry, generated network
installation and shared participant projection consume the current type.
Conversions occur once where existing AST/interface and participant types own
different map contracts. There is no compatibility alias or parallel reader.
This is storage and engine-invariant work, with no changeable policy hook.

The ignored `named_observation_admission_benchmark` passes before and after:
512 accepted observations with 2,304 named variables, one input and one parameter
take 99.499 ms and 80.044 ms respectively. Exact final bits, classification and
communication clock match. This bounded diagnostic includes result construction;
it is not a whole-tick speed claim. Eleven bridge checks and three focused
co-simulation checks pass, covering stale results, missing state, input order,
stable storage, command precedence, rejected physical sinks and no-op publication.

Both desktop binaries build with the regular profile and `-j 4`. The canonical
`cargo build --locked -p lunco-luncosim -j 4` passes in 39 seconds, SHA-256
`0b0301c799359a7b004546eae4421c4b8edb00a33ca3da34b9d86854637a8f7b`.
The existing unused stage-import warning remains.

Production qualification uses owned API ports 5188–5190. The variable-projection
scene passes seven checks, the missing-state scene passes its expected terminal
`modelica-variables-missing` verdict without panic, and the graphical metadata
scenario passes all 35 checks. Its final notification image is inspected and
diagnostics are clear. The one-off wrapper's scene-name comparison initially
included the negative verdict's fault description; the actual zero exit and
exact terminal PASS/source/fault were rechecked without repeating that test.
All owned gate sessions close normally.

The three normal desktop windows require full terrain readiness, five seconds
of soak and ten measured seconds with at least 240 fresh fixed ticks. Sources
remain unchanged within every run; diagnostics are clear and final images are
inspected. Each app closes. Another worktree's body-view session and browser
remain active, so these are contention-affected observations.

| Twin | Loading observation | Coupled tick median/p95 | Frame median/p95 |
|---|---:|---:|---:|
| Summer | fully usable 1.965 s | 3.412/3.912 ms | 8.518/10.205 ms |
| Tesla | fully usable 2.435 s | 1.633/2.030 ms | 4.323/6.683 ms |
| Griffin | first ready 14.036 s | 3.876/4.485 ms | 10.877/14.342 ms |

Griffin's 36.291-second landed milestone includes descent. Summer and Griffin
authored hashes match the preceding cohorts; Tesla is a newer source revision
and cannot support an isolated application comparison. Its known frame/datum HUD
message remains outside this qualification. Native counts and cache facts remain
in the receipts. These runs do not prove complete missions or clean performance
acceptance. All original loading, coupled-tick and graphics targets remain open.

Evidence stays ignored under `.cache/codex-evidence/performance-20261023/` in
`modelica-live-maps-*`, `summer-modelica-live-maps-normal`,
`tesla-modelica-live-maps-normal` and `griffin-modelica-live-maps-normal`.
Ten regenerable older CSV exports are removed after checking their retained
native captures, export receipts and summaries, recovering 14,588,378,402 bytes.
Source, build outputs and shared caches remain intact.
