# Native model diagram capture

The canonical Roadster `RoadsterPowertrain` icon shows tanks, the aggregate
rear valve and four rear nozzles, with a combined thrust readout. Twin commit
`691a85f` changes annotations only. Physical declarations and equations retain
SHA-256 `ec4fe51ae3b3aaa899ad1897c6d177c30148a987a0cb2102d9f42cf2dab4ab08`
up to the icon annotation.

## Shared navigation repair

`SetZoom` preserves `Viewport.target_center` and `PanCanvas` preserves
`target_zoom`. Reading the currently interpolated value cancelled an earlier
focus/pan request when commands arrived together. The rejected v34 take clips
the tank despite passing its timing verifier. A native screenshot on the new
build shows the complete four-nozzle diagram after one composed
`FocusComponent` / `SetZoom` request. The production `report_tank_inputs_v37` and `report_gas_flow_diagram_v37`
takes pass 480/420 native 60 fps image receipts. Raw opening/final frames
show the complete card after the composed focus/zoom commands.

## Typed presentation settings

`ConfigureDiagramPresentation` applies optional `show_controls` and
`use_authored_colors` fields at the workbench settings owner. The settings menu
dispatches that same command. Presentation actors use it rather than mutating
reflected resources directly. The v35 attempt correctly rejected that forbidden
write before recording any images. The new typed command passes the production
debug build. Both v37 takes execute that typed command from a presentation
actor, omit diagram controls, and retain the authored dark colors.

For document-backed model views, open and finish the native per-document view
before capture. The ordinary `modelica_analyze` view provides explicit document
ownership and can be framed to the model in the edit. Raw opening and final images were reviewed without loading indicators;
the accepted spatial crop is `[584,396,1744,820]` from 2560×1568.
The clip is an explicit analysis hold at simulation tick 120. This verifies
the diagram UI, not vehicle physics or hybrid ray tracing.

Evidence: marketing commit `fb725ba`,
`delivery/four_nozzle_capture_review.json`, and
`/home/rod/.cache/roadster-hybrid-v36-session.log`.
The first v36 attempt stalled after one 1280×720 frame; it was rejected.
The subsequent windowed v37 takes completed at 2560×1568. No rejected
frames are selected in the edit.
