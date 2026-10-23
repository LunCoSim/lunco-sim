# Native analysis presentation

Two shared UI repairs were checked in the regular tutorials application.

`LinePlotStyle.show_hover_values` defaults to true and is persisted per plot.
`ConfigureLinePlot` and the appearance menu expose the same setting. Both live
and historical plots pass it to egui_plot's existing hover controls; axes,
samples and navigation keep their normal owners. Film policy selects false.
No alternate plot renderer or pointer-position workaround is involved.

Root `Diagram(graphics=...)` DynamicSelect text previously became an editable
static label, losing its expression. Static text remains editable. Dynamic
text stays in the graphics layer and uses the existing Modelica icon painter
with the canvas's result/live-value snapshot. This is a rendering mechanism;
the model owns expressions, labels and values.

Validation on 2026-10-23:

- Two line-plot style/configuration tests passed.
- The regression test for dynamic diagram text passed: its fallback is not
  materialized as a static text node, while adjacent ordinary text is.
- The regular debug application built successfully.
- Native port 4387, binary SHA256
  `6fb5f3596919d4ba9d675d71b4b4163d3a37b4129d9dfa5281b5cc12cbb3d248`:
  the same pointer location shows a tooltip/crosshairs when enabled and neither
  when disabled. Both captures retain axis numbers and the calculated series.
- The native pressure experiment then showed `437.02 bar` and `350.62 bar`
  through DynamicSelect, matching its real-gas and ideal-gas outputs. The
  document's opening/projection notice cleared before the settled image.

Images reviewed in `.cache/`: `roadster-hover-enabled-v26-b.png`,
`roadster-hover-disabled-v26.png`, `roadster-pressure-card-v26-b.png`.
Build inputs and tests are retained locally under
`/home/rod/.cache/roadster-diagram-hover-build-input.json`,
`roadster-diagram-text-tests.log`, `roadster-plot-hover-tests.log`, and
`roadster-diagram-hover-build.log`.

This verifies UI behavior. It does not accept a finished film, every Modelica
annotation expression, or the experimental ray-tracing backend.
