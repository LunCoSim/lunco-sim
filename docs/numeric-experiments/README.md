# Numeric experiments — solver / model integration

This folder captures detailed write-ups of numerical experiments where we
diagnosed why a model wouldn't integrate, what we tried, what worked, and
what to remember next time. Each file is a session record, not a spec.

The goal is twofold:

1. **Don't re-derive the same fix.** When a stiff DAE or solver-config
   problem comes back, future-you reads the matching write-up and gets
   the working configuration immediately, plus the *why*.
2. **Surface design debt with concrete evidence.** Each report ends with a
   "TBDs / future work" section that links back to specific rumoca files
   and behaviours. That feeds the AGENTS.md task list.

## File naming

`YYYY-MM-DD-<short-topic>.md`. Date is when the diagnosis happened; the
file is immutable history, not a living doc.

## Structure

Each report has these sections (in order):

1. **Problem** — model + what failed, exact error text.
2. **Symptoms** — observable behaviour, repro recipe.
3. **Investigation** — what we tried, what we ruled out (failed
   hypotheses are as important as wins).
4. **Root cause(s)** — the actual diagnosis.
5. **Fix** — what changed (rumoca settings, model annotations, ...).
6. **Validation** — sweep results / numbers that prove it works.
7. **TBDs / future work** — design debt + concrete next-step ideas.

## Index

- [2026-05-28 — Lunar rover thermal model](2026-05-28-lunar-thermal.md):
  stiff radiative DAE failing at t=2.5e-7 across all solvers; root cause
  was FD-Jacobian degeneracy at the consistent-IC solve combined with
  insufficient retry budgets. Working configuration: TR-BDF2 + tol=1e-3
  + dt=3600 + new rumoca defaults. Scales linearly to multi-month
  horizons.
