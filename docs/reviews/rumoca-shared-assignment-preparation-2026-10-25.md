# Shared assignment preparation qualification

Status: native construction-only patch `45d66677bd1e287491279823c039e1da954880be`
is tested and published on LunCoSim/rumoca branch
`fix/shared-assignment-preparation`. The application pin is qualified for local integration; the separate mutable
workspace candidate remains held.

## Scope and ownership

The previously approved immutable assignment-preparation change is isolated
from the separate runtime sweep-workspace change. The latter remains on the
published `fix/shared-refresh-runtime-preparation` branch and retains its exact
replay investigation. The new candidate's sweep implementation and its shared
fixture helpers are byte-identical to application base `b825929b`.

`rumoca-ir-solve::SharedAssignmentSchedule` owns checked immutable scalar
programs, source provenance and per-program solver targets. The equivalence
check runs before operations move into the existing `ScalarProgramBlock` Arc
storage. Interpreter and Cranelift consume that same proven schedule instead
of copying operations and independently preparing the same proof. Generated C
consumes the current typed contract too. Program execution and target commits
preserve source order; mutable numerical state remains instance-owned.

Hook review: immutable proof construction and ownership are engine invariants,
not scenario policy. The application uses Rumoca's maintained facade and
introduces no backend, adapter or policy hook.

## Native evidence

The original fork branch is preserved. On the isolated branch, 128 focused
solver-runtime checks and five Cranelift assignment checks pass. The five
affected owner packages pass all-target/all-feature Clippy with warnings denied.
Native formatting and complete diff review pass; the DCO-signed-off commit is
pushed and its remote SHA verified.

The fixed twenty-model Tier 1 canary retains twelve strict OpenModelica 1.27.1
comparisons, with zero deviations, missing/skipped comparisons or simulation
timeouts. Six models stop in ToDAE; two receive EL005 structural-lowering
refusals before solver runtime. These match the previous combined patch's
counts. The focused run skips the full-cohort quality-delta gate; no runtime
baseline delta is asserted for the eight refusals, and no Tier 2 parity claim
is made. Native raw results remain ignored under
`../rumoca/target/msl/schedule-only-canary`.

## Application admission

All Rumoca-family source pins move together. The lockfile retains its complete
previous dependency graph, changing only the Rumoca Git source identity; the
locked regular build verifies that graph. Unrelated Windows-only resolution
churn is excluded. The normal optimization `target/` was removed during native
qualification, so the production artifact is rebuilt there with sccache and
four Cargo jobs.

The regular candidate is SHA-256
`7981e08be0dabd3f8a92d40bfe643f779d621720277e85e73246c37021b06495`.
Production gates pass on that artifact: 46 initialization/receipt checks,
20 source-target rejection/recovery checks, both expected terminal-fault gates,
21 retirement/duplicate-lifecycle checks, and all ten exact profiles (360
checks). References and tolerances are unchanged. The separate Summer Space
School Lunokhod physics fixture passes 102 authored assertions; Roadster's
launch verification passes eight requirements. Their 72 and 132 source hashes
respectively remain unchanged. These are bounded cases, not full-mission or
all-tricks acceptance.

## Production measurements

The first-fill Griffin run (owned API 5374) reaches readiness in 18.435518 s,
with a prepared-initializer disk miss and 3.871277 s initializer construction.
The warm run (API 5375) reaches readiness in 12.775131 s, with a 708.301 ms
prepared-initializer disk hit and 2.935069 s construction. Both retain all 271
source hashes, land at sleeping four-pad handoff, remain finite and unpaused,
and have no registered warning/error or script errors. The warm screenshot is
inspected. Its fresh coupled-tick p50/p95 is 3.393576/4.162381 ms. The prior
qualified pin's readiness observation was 14.134817 s; changing host load and
view prevent an isolated total-time or graphics speedup claim. Sub-second
loading and sub-ms coupled ticks remain open.

Both runs perform 16,482 native artifact lookups: 16,414 hits, 68 compiles,
zero rejected artifacts and zero evictions. Each owned session closes through
its API. Raw receipts, logs and source snapshots remain ignored under
`.cache/codex-evidence/performance-20261023/`.

## Diagnostic attribution

A separate warm Tracy run (owned API 5376, collector port 8448) uses artifact
`1e186ea291abd476d33e2928e6ef4e6b0fd49f8eac4090193d9538c9a53c0482`.
Its 270,722,782-byte capture has verified native framing and exports 3,666,220
zones. The selected initializer constructor spans 3.648040 s; 2,956 selected
CPU samples at the verified 1 ms period have no missing stacks. Remaining
leaves include scalar-operation cloning, term interning/keys, hashing,
Cranelift verification and native proof preparation. Sharing removes a specific
duplicate preparation, not all IR construction or proof costs. The instrumented
17.634606 s readiness is diagnostic only. All source hashes match. The collector
and app close, and the regular production artifact is restored to its exact
SHA above.

No mutable sweep change, Avian fix, reduced numerical check, altered integration
setting or new policy hook is part of this pin. This candidate's passing exact
profiles do not establish resolution of the separate intermittent articulated
first-step finding.

## Evaluator-selection diagnostic

A bounded production diagnostic forces Rumoca's supported interpreter policy
for all live constructors, then exits at first physics readiness. Its initializer
makes zero native lookups but constructs in 6.808034 s and spends 2.405787 s on
the first checked observation; readiness is 20.230421 s. The immediately
subsequent native control constructs in 3.209457 s, observes in 1.333 ms, and
reaches readiness in 15.381713 s with 16,414 hits and 68 compiles. Host load
qualifies totals, but removing JIT does not remove the expensive evaluation and
is not an accepted loading optimization. Both retain all 271 source hashes,
close owned APIs 5379/5380, and have clear registered diagnostics. They certify
startup only, not landing or complete replay.

The probe is removed completely. The regular restored artifact is
`74354134a2c684023108bdc0cd0d5b2f17e243cca8d20e0f78b84e19cb2839c4`;
its build has no warnings. Its application commit metadata includes the pin's
local integration. No evaluator-selection mechanism or policy change remains.
