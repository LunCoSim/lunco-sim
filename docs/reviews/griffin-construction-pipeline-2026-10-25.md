# Griffin construction pipeline

Status: application-owned stage separation passes focused and production
qualification. Dependencies remain unchanged. Constructor overlap is measured;
the normal loading observations do not establish an isolated overall speedup.

## Measured owner

The regular baseline `ffb5e39d` runs in owned PID 4079268/API 5355, reaches
physics readiness in 14.543 s and four-pad landing eligibility in 36.751 s.
All 271 before/admitted/after source hashes match; diagnostics are clear and
the process/port close. The initializer takes 3.276 s. Main propulsion,
attitude propulsion and actuation constructors take 0.742/1.128/0.923 s.
The preceding validated warm-cache capture shows these constructors running
consecutively, although the next prepared component is already available.

## Authoritative contract

`lunco-modelica-worker` owns command admission, immutable preparation dispatch,
and live publication. The worker reserves a request ID, domain worker lane and
publication slot before preparing its component. One preparation queue consumes
checked IR in submission order; a separate construction queue publishes the
corresponding success or original typed error in reserved order. A memory hit
uses the same publication queue. Shared lowering retains one position and lane
per admitted follower. Preparation completion can therefore launch independent
constructors without waiting for another constructor's publication.

The admitted build plan is retained through construction. Obsolete preparation
releases its publication slot after draining; live publication retains current
session and library-generation checks. Thread-affine construction, stepping and
retirement, existing worker counts, admission bounds, physics clocks, numerical
settings, checked observations and source-root fences retain their owners.
Duplicate completions fault at the worker instead of continuing with overwritten
results. No dependency source, simulation equations or authored scene changes.

Hook review: stage ordering, thread ownership, typed errors and publication
fences are engine invariants. No Twin-specific selection or lifecycle policy is
introduced, so no Rhai hook is involved.

## Verification

The focused scheduling suite passes thirteen tests. Its stage regression holds
the first live constructor unfinished, releases the second prepared component,
then finishes the later constructor with an error. Dispatch proceeds with its
reserved lane, while publication waits for the first result. The same check
covers a prepared-cache hit and a retired preparation releasing its slot.
The production initialization/receipt replacement gate passes 46 authored
checks, source-target rejection/replacement/recovery passes 20, and Twin
runtime/duplicate retirement passes 7/14. The negative initialization and
observation scenes terminate successfully on their expected
`modelica-initialization-failed` and `modelica-step-failed` faults. Original
diagnostic detail is retained. All ten unchanged exact-replay profiles pass
360 checks, including the altered-row discriminator. The reference SHA-256
remains `6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`.
Owned ports 5357–5363 close.

The final cleanup removes two unread primitive fields from `CompileWork`.
Scoped Clippy compiles library/tests and reports the same 26 existing unique
diagnostics outside the new stage mechanism; no waiver is added. The regular
rebuild emits no compiler warning. A post-cleanup `scene-4-serial` production
replay passes all 36 checks on owned API 5366, which closes. The full matrix
belongs to candidate `e3a87650`; cleanup changes no calculation or queue logic.

## Normal observations

Candidate `e3a87650`, PID 4099124/API 5356, reaches first readiness in 17.664 s.
Its initializer is 4.469 s versus baseline 3.276 s; prepared-IR lookup is
1.594 s versus 0.757 s. Additional browser workloads appear before the queue
change's relevant construction interval. This cohort is not an isolated loading
comparison. It lands normally and closes with unchanged source hashes.

The restored regular build `ca61f235`, PID 4115902/API 5365, reaches readiness
in 14.135 s and four-pad eligibility in 36.161 s. It completes the five-second
soak and ten-second / 240-fresh-tick window, sleeping, finite and unpaused,
with no registered warning/error or script error. All 271 before/admitted/after
source hashes match both other normal cohorts. The screenshot is inspected;
process and port close. Coupled median/p95 is 3.873/6.612 ms and frames
10.421/16.247 ms. Camera/contention limit those observations; the loading,
coupled-tick and graphics targets remain open.

## Native stage attribution

Tracy 0.14.1 / 1 kHz uses app 4110824/API 5364 and collector 4110772/port 8447.
The socket receipt matches both PIDs. Native framing verifies 319,791,071 bytes,
14,082 blocks and 319,734,737 payload bytes, matching rounded 304.92 MiB.
Native export reads all 4,859,575 zones. Source hashes match the normal cohort;
both processes and ports close.

Construction jobs 12/13 start at 39.534726/39.534803 s on distinct workers.
Their inclusive sum is 2.007599 s; their combined interval is 1.012375 s,
with 0.995223 s overlap. Early independent component constructors also overlap.
This directly verifies removal of the serial wait; summed inclusive work is
not an end-to-end product speedup.

The capture's 35.491 s readiness observation includes a prepared-IR miss:
initializer lowering alone takes 5.688 s. The prepared disk cache retains 32
records, and the preceding matrix exercises other models; the capture does not
identify an individual eviction writer. Native compiler artifacts
are still warm (16,414 initializer hits / 68 compilations). These are distinct
cache conditions; no overall comparison with the earlier warm-prepared capture
is claimed. The ordinary production build is restored. Raw data stays ignored.
