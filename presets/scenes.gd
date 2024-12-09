#
# Please do not edit anything in this script
#
# Just use the editor to change everything you want
#
extends Node

var scenes: Dictionary = {"AiChat":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/ai/AiChat.tscn"},"BoolConditionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/condition_editors/BoolConditionEditor.tscn"},"ConditionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/condition_editors/ConditionEditor.tscn"},"ContextMenu":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/ContextMenu.tscn"},"FloatConditionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/condition_editors/FloatConditionEditor.tscn"},"FlowChartLine":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/flowchart/FlowChartLine.tscn"},"FlowChartNode":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/flowchart/FlowChartNode.tscn"},"FutureLunarMissions":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/future_lunar_missions/FutureLunarMissions.tscn"},"IntegerConditionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/condition_editors/IntegerConditionEditor.tscn"},"OverlayTextEdit":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/ui_design_tool/scenes/OverlayTextEdit.tscn"},"QuadPlaneLOD":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/quad_plane_test/QuadPlaneLOD.tscn"},"QuadPlaneLODDemo":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/quad_plane_test/QuadPlaneLODDemo.tscn"},"QuadSphere":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/future_lunar_missions/QuadSphere.tscn"},"RespawnIfFallen":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/ability/RespawnIfFallen.tscn"},"SignalDebugger":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/SignalVisualizer/Debugger/SignalDebugger.tscn"},"SolarEnergyModel":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/solar_energy/SolarEnergyModel.tscn"},"StackItem":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/src/debugger/StackItem.tscn"},"StackPlayerDebugger":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/src/debugger/StackPlayerDebugger.tscn"},"StateMachineEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/StateMachineEditor.tscn"},"StateNode":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/state_nodes/StateNode.tscn"},"StateNodeContextMenu":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/StateNodeContextMenu.tscn"},"StringConditionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/condition_editors/StringConditionEditor.tscn"},"Toolbar":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/ui_design_tool/scenes/Toolbar.tscn"},"TransitionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/transition_editors/TransitionEditor.tscn"},"TransitionLine":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/transition_editors/TransitionLine.tscn"},"ValueConditionEditor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/imjp94.yafsm/scenes/condition_editors/ValueConditionEditor.tscn"},"_auto_refresh":true,"_auto_save":true,"_ignore_list":[],"_ignores_visible":true,"_sections":[],"astronaut":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://content/animated-astronaut-character-in-space-suit-loop/astronaut.tscn"},"avatar":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/avatar/avatar.tscn"},"avatar_ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/avatar/avatar_ui.tscn"},"blank-facility":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/facilities/blank-facility.tscn"},"bullet":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://content/gobot/bullet/bullet.tscn"},"character":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/entities/character.tscn"},"character-controller":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/character/character-controller.tscn"},"character-input-adapter":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/character/character-input-adapter.tscn"},"character-ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/character/character-ui.tscn"},"chat-ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/chat/chat-ui.tscn"},"console":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/console.tscn"},"console_logs":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/console_logs/console_logs.tscn"},"content":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://content/content.tscn"},"crt_effect_layer":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/screen_crt_effect/crt_effect_layer.tscn"},"earth":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/solar_system_planets/earth.tscn"},"editor":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/text-editor/editor.tscn"},"exp_history":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/history_manager/exp_history.tscn"},"exp_history_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/history_manager/exp_history_item.tscn"},"exp_key_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/keyboard_shortcuts/exp_key_item.tscn"},"exp_key_mapper_2":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/keyboard_shortcuts/exp_key_mapper_2.tscn"},"exporter_2":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/exporter_2.tscn"},"expression_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/expression_monitor/expression_item.tscn"},"expression_monitor2":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/expression_monitor/expression_monitor2.tscn"},"globe":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/models/globe.tscn"},"gobot":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://content/gobot/gobot.tscn"},"help-spacecraft":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/help-spacecraft.tscn"},"help_bar":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/console_ui/help_bar.tscn"},"hint":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/hints_list/hint.tscn"},"hints_list":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/hints_list/hints_list.tscn"},"ignore_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/ignore_item.tscn"},"input_area":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/input_field/input_area.tscn"},"label":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/label.tscn"},"langrenus_crater_map":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://content/maps/Langrenus/langrenus_crater_map.tscn"},"living-facility":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/facilities/living-facility.tscn"},"log_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/screen_notifier/log_item.tscn"},"log_overlay":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/native_logger/log_overlay.tscn"},"log_view_tag":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/native_logger/log_view_tag.tscn"},"logger_view":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/native_logger/logger_view.tscn"},"lunar_globe":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/solar_system_planets/lunar_globe.tscn"},"lynx_window_2":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/common/lynx_window2/lynx_window_2.tscn"},"machine_role_container_ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/machine_role_container_ui.tscn"},"main":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://main.tscn"},"main_menu":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/menu/main_menu.tscn"},"main_showcase":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://main_showcase.tscn"},"mbse":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/mbse/mbse.tscn"},"menu":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/menu.tscn"},"met":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/met.tscn"},"mini_repl_2":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/mini_repl_2.tscn"},"monitor_group_ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/expression_monitor/monitor_group_ui.tscn"},"monitor_groups_ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/expression_monitor/monitor_groups_ui.tscn"},"moon":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/solar_system_planets/moon.tscn"},"nft-create-popup":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/nft-create-popup.tscn"},"nft-sphere":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/facilities/nft-sphere.tscn"},"operator":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/entities/operator.tscn"},"operator-controller":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/operator/operator-controller.tscn"},"operator-input-adapter":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/operator/operator-input-adapter.tscn"},"operator-model":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/models/operator-model.tscn"},"operator-ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/operator/operator-ui.tscn"},"orientation-system":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://components/orientation-system/orientation-system.tscn"},"panku_console_ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/console_ui/panku_console_ui.tscn"},"repl":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/interactive_shell/console_ui/repl.tscn"},"resident_logs":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/screen_notifier/resident_logs.tscn"},"rigid-body":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/entities/rigid-body.tscn"},"rocket-engine":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://components/rocket-engine/rocket-engine.tscn"},"rocket-plume-2d":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/models/rocket-plume/rocket-plume-2d.tscn"},"rocket-plume-fog":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/models/rocket-plume/rocket-plume-fog.tscn"},"row_bool":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_bool.tscn"},"row_button":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_button.tscn"},"row_color":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_color.tscn"},"row_comment":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_comment.tscn"},"row_enum":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_enum.tscn"},"row_float":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_float.tscn"},"row_group_button":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_group_button.tscn"},"row_int":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_int.tscn"},"row_range_number":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_range_number.tscn"},"row_read_only":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_read_only.tscn"},"row_string":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_string.tscn"},"row_vec_2":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/data_controller/exporter/row_vec_2.tscn"},"rsct":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/supply_chain_modeling/rsct.tscn"},"scene_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/scene_item.tscn"},"scene_list":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/scene_list.tscn"},"scene_manager":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/scene_manager.tscn"},"signal_graph_node":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/SignalVisualizer/Visualizer/signal_graph_node.tscn"},"signal_graph_node_item":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/SignalVisualizer/Visualizer/signal_graph_node_item.tscn"},"signal_visualizer_dock":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/SignalVisualizer/Visualizer/signal_visualizer_dock.tscn"},"simulation":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/simulation/simulation.tscn"},"smooth_scroll":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/common/smooth_scroll/smooth_scroll.tscn"},"solar-power-station-facility":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/facilities/solar-power-station-facility.tscn"},"spacecraft-controller":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/spacecraft/spacecraft-controller.tscn"},"spacecraft-input-adapter":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/spacecraft/spacecraft-input-adapter.tscn"},"spacecraft-ui":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://controllers/spacecraft/spacecraft-ui.tscn"},"sphere":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/future_lunar_missions/sphere.tscn"},"spring-arm-camera":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/lunco-cameras/spring-arm-camera/spring-arm-camera.tscn"},"starship":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://content/starship/starship.tscn"},"stopwatch":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/stopwatch.tscn"},"sub_section":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/scene_manager/sub_section.tscn"},"sun":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/solar_system_planets/sun.tscn"},"texture_viewer":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://addons/panku_console/modules/texture_viewer/texture_viewer.tscn"},"tutorial":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/widgets/tutorial.tscn"},"ui-attacher":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/compositions/ui-attacher.tscn"},"universe":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://core/space/universe.tscn"},"web3-server":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/web3/web3-server.tscn"},"yarm":{"sections":[],"settings":{"All":{"subsection":"","visibility":true}},"value":"res://modules/yarm/yarm.tscn"}}
