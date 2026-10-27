# Rumoca torn-sweep workspace

The user-authorized native change is published as
`eca3948416fa90d8695e2f91557ee9db18dbe75d` on LunCoSim/rumoca branch
`fix/torn-sweep-workspace`. The
[current integration review](rumoca-refresh-runtime-preparation-2026-10-25.md)
owns implementation, qualification and production timing status.

The source-matched Summer capture uses owned PID 2811282, API 5306 and Tracy
0.14.1 at 1 kHz. Its final 240 completed `FixedMain` instances exclude twenty
screenshot/shutdown ticks and pass fifteen native duration/counter checks.
Framing and native exports pass. Selected `modelica_integrate_macro_step`
intervals contain 231 CPU samples, with no missing stacks; 35 leaf samples
resolve to `_mi_page_malloc_zero`. This establishes allocation cost, without
assigning every allocation to one caller or promising a proportional gain.

Rumoca's existing per-runtime torn-sweep owner now retains raw residuals, dense
native outputs and independent debug-reference buffers. Every live entry is
refilled; runtime clones own separate storage. Independent per-row replay,
complete f64 bit comparisons, decline decisions and numerical checks remain.
The public Model Exchange adapter stays the application boundary; no solver
implementation or scratch cache is duplicated in application Rust.
