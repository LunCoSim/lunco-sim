# Reviews & standing issues

Two kinds of file live here, and the difference is the lifecycle.

| Pattern | What it is | Lifecycle |
|---|---|---|
| `open-<name>.md` | A known defect or accepted limitation that is **not fixed**, kept deliberately visible | Stays until the issue is closed, then deleted |
| `YYYY-MM-DD-<name>.md` | An audit report from a review pass ([`skills/deep-audit`](../../skills/deep-audit/SKILL.md)) | **Deleted once its findings land** — git remembers it |

A closed report left in place reads as an open problem list, and the next
reviewer wastes a pass re-confirming it. Anything worth keeping from a report is
either a finding that is still open — which becomes an `open-*.md` — or a design
lesson, which belongs in the architecture doc for that subsystem.

## Standing issues

- [`open-2026-09-10-general-live-modeling-capability-audit.md`](open-2026-09-10-general-live-modeling-capability-audit.md) —
  the single current audit for generic Editor/AI authoring. It records the
  implemented substrate and the remaining human component workflow, candidate
  preview, diagnostic navigation, and conditional recipe-freshness gaps.
- [`open-200fps-performance-handover.md`](open-200fps-performance-handover.md) —
  the Apollo High-quality frame loop is below the stable 200 FPS target; the
  handover records the Tracy evidence and owner-first optimization plan.
- [`open-400fps-performance-handover.md`](open-400fps-performance-handover.md) —
  the current 400-FPS target, change-driven globe LOD implementation, and the
  measured upstream/render blocker.
- [`open-rbac-not-enforced.md`](open-rbac-not-enforced.md) — **the project does
  not enforce access control.** Trusted LAN only; never expose a host to an
  untrusted network.
- [`open-2026-07-27-sandbox-windows-nightly.md`](open-2026-07-27-sandbox-windows-nightly.md) —
  defects found in the `sandbox-windows-x86_64` nightly during a tester session.

## Report hygiene

The superseded live-modeling handover, FLIP capability, roadmap, parametric
component design/plan, rover-authoring, and focused Editor-efficiency reports
were removed after their findings were reconciled into the current audit.
Closed implementation history remains in git; it is not kept as an open
problem list.
