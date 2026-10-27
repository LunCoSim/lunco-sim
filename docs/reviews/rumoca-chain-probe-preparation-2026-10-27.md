# Avoid preparing discarded chain candidates

Status: proposed native Rumoca change; implementation and publication need
explicit approval. The application remains at Rumoca `eca3948416fa`.

## Current evidence

Application `c4841c194` uses its regular profile with opt-in Tracy, adjacent
Tracy 0.14.1 tools and 1 kHz CPU sampling. Owned application PID 1021015/API
5530 connects to collector PID 1020998/port 8462. The Griffin driver qualifies
landing and settled operation; all 271 source hashes match before admission,
at qualification and after exit. The final image shows the lander, terrain,
shadow and four-pad handoff HUD. Both owned processes and ports close.

The saved capture is 330,300,113 bytes: six header bytes, 14,935 four-byte
block lengths and 330,240,367 payload bytes, agreeing with the collector's
314.94 MiB report. Native zone and CPU exports succeed. Three native CSV rows
contain commas inside preparation-span names; splitting from the six trailing
columns recovers all 434,320 Modelica rows. These three rows are preparation
jobs, not the exact constructor or settled integration zones below.

Initializer construction takes 4.192 s, with 16,482 native cache hits and no
compilations or evictions. Exact-zone selection contains 3,524 CPU samples,
zero missing stacks and one constructor zone. Application addresses are
resolved against its recorded ELF maps and symbol table. Inclusive sample
groups overlap and must not be summed:

| Work | Samples |
| --- | ---: |
| Cranelift and SHA-256 | 776 |
| Shared-value preparation | 683 |
| LinearOp cloning | 239 |
| Tensor-affine preparation | 178 |
| Assignment-producer lookup | 150 |

2,596 samples have only one native frame. These observations establish owner
costs but cannot assign all shared-value work to the chain-probing loop, or
all cloning to whole-model copies. Warm executable reuse still requires
construction and compiler-input hashing. These diagnostic timings are not
product performance or an isolated comparison with previous captures.

For the last 240 complete fixed ticks before the twenty-tick shutdown margin,
CSV worker 55 spends 1,885.752 ms in 479 fully contained integration zones and
328.703 ms collecting observations. That lane services multiple models;
its aggregate is not a per-model or complete coupled-step duration. Exact-zone
sampling selects 1,888 integration samples on this lane, including 233 leaf
samples in the independent torn-sweep agreement check. That check stays intact.
The 302 observation samples include name cloning, hashing and refresh checks;
dropping observations or validation is not an optimization proposed here.

Raw captures, exports, symbols and receipts remain ignored in
`.cache/codex-evidence/performance-20261027/current-tracy/` and
`scripts/perf/captures/griffin-torn-workspace-20261027.tracy`.

## Native owner and proposed change

`rumoca-eval-solve::PreparedScalarProgramBlock::torn_sweep_runs` extends a run
through `extends_run`. Each attempt calls `target_isolation_chain_program`
and discards the returned program after checking `is_some()`. That constructor
validates target dependencies, failable operations, prefix order and register
materialization, then invokes `share_program_values`. The sharing operation
always returns a program; its result cannot change the probe's admission.
The accepted run is constructed again by both the prepared sweep and generated
C consumers before execution.

Separate the existing chain construction from shared-value optimization inside
this authoritative owner. Membership probes use the same checked construction
without optimizing a program that will be discarded. Executable programs
still run shared-value derivation and its equivalence proof. Preserve every
admission check, final proof, numerical guard, error/decline decision and
floating-point operation. Keep a single construction implementation and update
all consumers; add no application cache, alternate solver or compatibility API.

This is immutable compiler preparation, with no lifecycle policy or Rhai hook.
The expected saving is discarded optimization work, not fewer model equations.
Its magnitude remains unmeasured until a focused native comparison is run.

## Required qualification

Establish an unchanged-input native baseline for complete run discovery plus
final preparation, covering long admissible chains, split runs, target
dependence, decreasing prefixes and failable operations. Require identical run
membership and final programs, including refusals. Reuse the native chain,
failable-operation and generated-C checks and fixed MSL canary. Retain the
runtime independent per-row agreement checks.

Only publish and pin a candidate after those gates pass. Then use the regular
production binary for the bounded coupled/replay checks and a source-matched
Griffin landing window. Compare cold/warm preparation and native cache counts
separately; verify rendering and owned-session closure. Loading and complete
coupled-step targets remain open.
