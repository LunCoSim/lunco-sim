# Historical diagram presentation

The Modelica plot domain owns `SetPlotScrubTime`. It writes the existing
`PlotPanelStates` historical cursor and active plot. The diagram snapshot reader
already resolves that cursor against the selected experiment's real samples;
the command adds no second timeline, sample store or physics playback path.
Unknown plot IDs and nonfinite times reject through the typed command result.
Omitting the time clears the cursor. This is a generic interaction setter,
not a changeable experiment-selection policy, so no new policy hook is needed.

`DiagramPresentationSettings.show_hover_values` belongs to the shared theme and
settings owner. The normal default is true. The existing presentation command
and settings checkbox update it; component and wire painters read the same
published preference. Authored values remain visible. This closes the case
where the OS pointer can cover an otherwise clean recorded diagram with a large
foreground hover card.

Validation: the production UI host rejected an unknown plot, and native offline
diagram captures replayed a completed experiment's changing valve, flow, tank
and thrust values. Directed frame review confirmed no hover cards with the
preference disabled. `assets/scripting/tests/test_plot_scrub_time.rhai` checks
finite selection, unknown-ID rejection, clearing and no physical-clock advance.
