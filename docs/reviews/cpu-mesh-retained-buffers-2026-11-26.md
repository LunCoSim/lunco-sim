# Retained native CPU mesh preparation and instance bytes

The maintained Bevy revision `c7582e05fadde13b9ebd9247a883d4b0edab80d2`
retains prepared uniform payloads and compact independent-input keys in the
existing generation-aware dense mesh inventory. Identical current and previous
transform bits and flags preserve preparation across extraction notifications.
Material binding, mesh allocation, mesh identity, metadata, lightmap, skin,
morph, tag and batchability facts are checked at the same render preparation
boundary as their actual consumers. Row removal moves both vectors with the
instance; owner reset invalidates the inventory.

Prepared elements carry a process-wide immutable data revision. Native CPU
batches borrow the same typed uniform through the existing `GpuArrayElement`
contract. Storage buffers preserve encoded shader-layout bytes and revisions at
their existing batch indices; changed slots update those bytes and a single
queue write covers the dirty extent. GPU allocation growth uploads the complete
active array. Unversioned providers still encode freshly supplied data and the
dynamic-uniform path retains its owned array and dynamic offsets. Native phase
order, eligibility, draw indices, GPU preprocessing and shader layout are
unchanged. This is not cross-pass range interning.

## Relative production qualification

Factory Site Layout was run in three regular native desktop sessions using the
same authored Twin revision, Avatar camera, target dimensions, render quality,
simulation settings, Tracy version and filter. The two reference runs used the
previous published pins; the candidate used the local source that was formatted
and published at the revision above. The bracket compares equally sized settled
render-frame cohorts against the mean of both reference runs.

| Native work | Candidate / bracket reference |
|---|---:|
| Uniform preparation, mean zone work | 79% |
| Native binned phase preparation, mean zone work | 87% |
| Instance upload, mean zone work | 96% |
| Preparation, all native CPU phases and upload | 87% |
| Combined work including extraction | 89% |
| Render schedule, mean (observational) | 96% |
| Application frame-rate ratio (instrumented, observational) | 101% |
| Application p95 frame time (instrumented, observational) | 100% |

Exact same-thread sampled CPU work from the first reference/candidate pair
independently shows lower preparation, opaque batching and upload work. The
small sorted-phase cost increased; the combined measurement includes it and
the extra extraction comparison work. Tracy timings do not establish a clean
uninstrumented FPS gain.

The stationary viewport agreed in over 99.99% of pixels between reference and
candidate. The native `shared_shadow_allocation` and `vertex_shadow_alignment`
fixtures both passed their authored scenario verdicts and positive green/cyan
pixel probes with no red disagreement. Focused native renderer and PBR tests
covered retained revisions, slot reorder, unversioned values, shader encoding,
dynamic offsets, exact transform invalidation, independent input changes, dense
swap/removal and owner retirement.

All sessions reached ready simulation with no runtime diagnostic errors or
warnings. The Factory source hits its existing global telemetry channel cap;
the reference repeats retained the same tested named groups, while the local
path-override candidate retained the same group counts but a different subset.
The renderer does not own channel admission: the physics recorder admits new
history in ECS body-query order until that cap. The final manifest and lockfile
pin only the two published Bevy owners, with all other lockfile selections
unchanged. Locked Cargo metadata resolves their source files byte-for-byte to
the formatted fork commit. A rebuilt production executable using those exact
Git pins retained precisely the same tested telemetry group paths, units and
counts as both references and agreed in over 99.99% of viewport pixels. That
run overlapped unrelated compiler/linker workloads; it qualifies the actual
dependency cohort's behavior, not an isolated FPS improvement.

The matched captures, exact diagnostic executables, build and run receipts,
sample exports, relative bracket, and visual gates are retained under
`target/perf/retained-instance-20261126/`.
