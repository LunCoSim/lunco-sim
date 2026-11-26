# Change-driven native CPU mesh extraction

The native PBR fork revision `e104a5d60f4aa6c8a724a73bf8e76981d8b456a6`
retains the dense CPU mesh inventory across frames. Component changes update
the affected row; component removal reconciles the final source state and
retired identities repair the dense index. The material owner publishes
association changes and retirements, and changed binding tables refresh
retained slots. Frame-scoped motion flags are cleared on their previous
owners. Initial admission and recovery cover unchanged main-world sources.
The earlier borrowed batch-data contract remains unchanged.

## Relative production evidence

Both captures used Factory Site Layout at the same source revision, native
desktop, stationary Avatar view, matching Tracy version and filter, and equally
sized settled render-frame cohorts. The application reference was `36a477586`
with PBR `4cea221a4`; the candidate used the local source of the published
incremental PBR revision. Concurrent compilation varied between captures, so
the overall render-schedule timing is not an isolated whole-frame speedup.

| Native work | Candidate/reference |
|---|---:|
| Mesh extraction, sampled CPU work | 58% |
| Mesh extraction, mean zone work | 52% |
| Mesh extraction and native material extraction/sweeps, mean zone work | 56% |
| Native material bookkeeping alone, mean zone work | 109% |
| Mesh uniform preparation, sampled CPU work | 94% |
| Render-schedule median (observational) | 75% |

The combined extraction result includes all material extraction and sweep
systems and the new begin-publication system; it is not a reduction claimed by
shifting work into an unmeasured owner. The stationary viewport images agreed
in 99.94% of pixels, with no visible missing geometry and only small remaining
pixel differences. Both sessions had ready simulation, clear runtime diagnostics
and identical telemetry channel coverage.

The focused native ECS tests passed the full extraction lifecycle: visibility,
mesh/transform/marker removal, same-frame replacement, material association and
binding changes, transient motion flags, dense index repair, owner recreation and
entity-generation reuse. The prior dense uniform regression remains green.
The production `shared_shadow_allocation` and `vertex_shadow_alignment` graphics
fixtures passed their authored verdicts and positive pixel probes with no red
disagreement. All comparisons preserve native visibility, shadows and materials;
physics cadence and telemetry admission are unchanged.

The actual production profiles were diagnostic Tracy builds running on the
regular desktop. The reference and candidate binaries, captures, receipts and
`relative.json` were recorded under the ignored
`.cache/cpu-extraction-evidence/` directory, which was subsequently cleared by
workspace disk cleanup. The published Git pin resolves to
the same candidate source after formatting; a fresh ordinary release build was
not qualified because of the remaining disk-space constraint. See the
[retained instance-buffer review](cpu-mesh-retained-buffers-2026-11-26.md) for
the change-driven uniform and upload owner.
