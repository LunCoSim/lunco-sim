class_name LCCommandConsole
extends Control

## In-game console for sending commands to entities.
## Syntax: /target command_name arg1=val1 arg2=val2

@onready var input_field: LineEdit = $VBoxContainer/HBoxContainer/LineEdit
@onready var output_log: RichTextLabel = $VBoxContainer/RichTextLabel
@onready var autocomplete_list: ItemList = $VBoxContainer/AutocompleteList

var history: Array[String] = []
var history_index: int = -1

func _ready():
	output_log.append_text("[color=gray]LunCo Command Console ready. Type '/help' for info.[/color]\n")
	input_field.text_submitted.connect(_on_text_submitted)
	input_field.text_changed.connect(_on_text_changed)
	autocomplete_list.hide()
	
	# Register a global shortcut
	set_process_input(true)

func _input(event):
	if event.is_action_pressed("toggle_console"):
		_toggle_console()
		# Mark as handled to prevent backtick from entering LineEdit
		get_viewport().set_input_as_handled()

func _toggle_console():
	visible = !visible
	var manager = get_tree().get_first_node_in_group("ui_display_manager")
	
	if visible:
		input_field.grab_focus()
		if manager: manager.active_display = "console"
	else:
		input_field.release_focus()
		if manager and manager.active_display == "console":
			manager.active_display = "none"

func _on_text_changed(new_text: String):
	if new_text.begins_with("/") and new_text.count(" ") == 1 and new_text.ends_with(" "):
		# Just typed "/target ", show commands
		var target = new_text.substr(1).strip_edges()
		_list_commands_for_target(target)

func _on_text_submitted(text: String):
	if text.is_empty(): return
	
	# Prevent the backtick from being in the history if it was just pressed
	if text.ends_with("`") or text.ends_with("~"):
		text = text.substr(0, text.length() - 1)
	
	history.append(text)
	history_index = -1
	input_field.clear()
	# Re-grab focus so user can type next command immediately
	input_field.call_deferred("grab_focus")
	
	output_log.append_text("[color=green]> %s[/color]\n" % text)
	_process_command(text)

func _process_command(text: String):
	if text == "/help":
		_show_help()
		return
		
	if text.strip_edges() == "/list":
		_list_targets()
		return

	if not text.begins_with("/"):
		output_log.append_text("[color=red]Error: Commands must start with /[/color]\n")
		return
		
	# Regex for parsing: /"Target With Spaces" Command Arg=Val
	var regex = RegEx.new()
	regex.compile("([\"'])(?:(?=(\\\\?))\\2.)*?\\1|\\S+")
	var matches = regex.search_all(text.substr(1))
	
	if matches.size() < 1:
		return
		
	var target = matches[0].get_string().erase(0, 0).replace("\"", "").replace("'", "")
	
	if matches.size() < 2:
		_list_commands_for_target(target)
		return
		
	var cmd_name = matches[1].get_string().to_upper()
	var args = {}
	
	for i in range(2, matches.size()):
		var arg_str = matches[i].get_string()
		var arg_parts = arg_str.split("=")
		if arg_parts.size() == 2:
			args[arg_parts[0]] = _parse_value(arg_parts[1])
		else:
			args["value"] = _parse_value(arg_str)
			
	var cmd = LCCommand.new(cmd_name, NodePath(target), args, "console")
	var result = LCCommandRouter.dispatch(cmd)
	
	if result is String and (result.contains("not found") or result.contains("does not implement")):
		output_log.append_text("[color=red]%s[/color]\n" % result)
	else:
		output_log.append_text("[color=gray]Result: %s[/color]\n" % str(result))

func _parse_value(val: String) -> Variant:
	val = val.replace("\"", "").replace("'", "")
	if val.is_valid_float():
		return val.to_float()
	if val.to_lower() == "true": return true
	if val.to_lower() == "false": return false
	return val

func _show_help():
	output_log.append_text("Available commands:\n")
	output_log.append_text("  /help - Show this help\n")
	output_log.append_text("  /list - List commandable targets\n")
	output_log.append_text("  /[target] [command] [args...] - Send command\n")
	output_log.append_text("For names with spaces, use quotes: /\"Starship 2\" SET_THRUST 0.5\n")

func _list_targets():
	var defs = LCCommandRouter.get_all_command_definitions()
	output_log.append_text("Commandable Targets:\n")
	if defs.is_empty():
		output_log.append_text("  [color=yellow](None found. Ensure entities have LCCommandExecutor nodes and are in the 'CommandExecutors' group.)[/color]\n")
	else:
		for target in defs:
			output_log.append_text("  [b]%s[/b]\n" % target)

func _list_commands_for_target(target: String):
	var defs = LCCommandRouter.get_all_command_definitions()
	var target_defs = defs.get(target)
	
	if not target_defs:
		# Try fuzzy match or actual NodePath
		for key in defs:
			if key.to_lower() == target.to_lower():
				target_defs = defs[key]
				break
				
	if target_defs:
		var cmd_list = []
		for c in target_defs:
			cmd_list.append(c.name)
		output_log.append_text("Available commands for [b]%s[/b]: %s\n" % [target, ", ".join(cmd_list)])
	else:
		output_log.append_text("[color=red]Target '%s' not found or has no commands.[/color]\n" % target)
