# Compact storage for admitted input transactions

`lunco-port-core` owns both live input validation and atomic application.
`InputWriteTransaction` stages under an immutable view of one exclusive World
borrow. After validation and duplicate detection, its private staged record
retains the commit callback, entity, name, slot and f64 value. Admission
metadata is released; it cannot change before these callbacks execute.
Every transaction clears its reusable storage before staging and after commit
or rejection. No retained record is exposed as permission for a later write.

The real consumers are co-simulation initialization and live input exchange.
Each target still validates the live precedence winner, topology revision,
slot, metadata, writability and scalar range. Staging order, duplicate checks,
all-or-none rejection and commit order remain unchanged. Independently retained
`PreparedPortWrite` values keep their metadata and full commit-time revalidation.
This storage invariant belongs to the registry; no policy hook or dependency
change is needed. No public interface changes.

## Evidence

The qualified Griffin capture described in the
[constructor review](rumoca-chain-probe-preparation-2026-10-27.md) supplies 240
exact `cosim_exchange_write` zones from its settled selection. Native export
selects 372 CPU samples, no missing stacks, and the verified 1 ms sample
period. The selected zones have median/p95 1.456/2.255 ms. Resolved-write preparation accounts for 58 leaf samples; staging,
component access and shader name lookup also appear. Unresolved libc frames
are not assigned to a particular copy or comparison operation. The experiment
therefore measures complete transactions rather than claiming that every
preparation sample is metadata movement.

All twelve focused input checks/diagnostics pass, including live metadata,
owner precedence, stale slots, duplicate rejection, atomic failure and buffer
reuse. Ten alternating independent executable pairs run the existing
`multi_port_transaction_diagnostic`. Each case executes 2,048 transactions
over 32 entities behind five absent higher-precedence owners, and verifies
exact final scalar bits:

| Inputs per entity | Baseline median | Candidate median | Improvement |
| --- | ---: | ---: | ---: |
| 1 | 12.737 ms | 12.198 ms | 4.23% |
| 8 | 110.542 ms | 104.812 ms | 5.18% |
| 32 | 448.736 ms | 431.087 ms | 3.93% |

Snapshots have independent file identities and stable, distinct hashes. No
Cargo build overlaps these pairs; other host workloads are recorded. These
fixture measurements are not complete coupled-step acceptance.

## Production qualification

The locked regular build finishes in 3m05s with normal target, sccache and four
jobs. Owned API 5533 passes eleven authored atomic-exchange/fail-stop checks,
then all ten exact replay profiles with 360 assertions. The reference SHA-256
remains `6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`.
Every finite test exits and releases its port. No tolerance, reference, solver
iteration or precision changes are made.

Griffin PID 1140590/API 5534 preserves all 271 source hashes at model `2b3c0a9`
and passes the settled landing window. The lander is sleeping and finite at
four-pad handoff, simulation remains unpaused and registered diagnostics have
zero warnings/errors. The inspected image shows the terrain, lander, shadow
and handoff HUD. The owned process exits and its port is released.

This launch observes 27.940 s physics readiness. The initializer's prepared IR
misses its disk cache despite the same source and compiler identities as the
previous qualified run; lowering takes 5.043 s. Native construction takes
5.207 s with all 16,482 kernels hitting and zero compilation/eviction. The
disk-miss cause is not established. Existing retention already marks cache hits
as used and serializes publication/pruning through the storage owner; no
additional cache mechanism is introduced.

Two foreign rendering sessions and video encoding are recorded. Fixed-schedule
service median/p95 is 5.238/7.558 ms, MainPropulsion worker mean is 8.334 ms,
and sampled frames are 18.443/23.261 ms. These overlapping intervals cannot
be added; fixed-schedule service is not the complete coupled step. Cache state
and contention prevent an isolated whole-scene comparison. The fixture gain
does not establish faster Griffin loading or product target acceptance.

Raw logs, snapshots and comparisons stay ignored in
`.cache/codex-evidence/performance-20261027/compact-transactions/`.
Loading, complete coupled physics and graphics targets remain open.
