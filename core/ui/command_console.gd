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
	autocomplete_list.hide()
	
	# Register a global shortcut or just keep it active if visible
	set_process_input(true)

func _input(event):
	if event.is_action_pressed("toggle_console"):
		visible = !visible
		if visible:
			input_field.grab_focus()

func _on_text_submitted(text: String):
	if text.is_empty(): return
	
	history.append(text)
	history_index = -1
	input_field.clear()
	
	output_log.append_text("[color=green]> %s[/color]\n" % text)
	_process_command(text)

func _process_command(text: String):
	if text == "/help":
		_show_help()
		return
		
	if text == "/list":
		_list_targets()
		return

	if not text.begins_with("/"):
		output_log.append_text("[color=red]Error: Commands must start with / (e.g., /rover1 set_motor value=0.5)[/color]\n")
		return
		
	var parts = text.substr(1).split(" ", false)
	if parts.size() < 2:
		output_log.append_text("[color=red]Error: Missing command name. Syntax: /target command [args...][/color]\n")
		return
		
	var target = parts[0]
	var cmd_name = parts[1]
	var args = {}
	
	for i in range(2, parts.size()):
		var arg_parts = parts[i].split("=")
		if arg_parts.size() == 2:
			args[arg_parts[0]] = _parse_value(arg_parts[1])
		else:
			args["value"] = _parse_value(arg_parts[0]) # Default 'value' if only one part
			
	var cmd = LCCommand.new(cmd_name, NodePath(target), args, "console")
	var result = LCCommandRouter.dispatch(cmd)
	
	if result is String and (result.contains("not found") or result.contains("does not implement")):
		output_log.append_text("[color=red]%s[/color]\n" % result)
	else:
		output_log.append_text("[color=gray]Result: %s[/color]\n" % str(result))

func _parse_value(val: String) -> Variant:
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
	output_log.append_text("Example: /Rover SET_MOTOR value=0.5\n")

func _list_targets():
	var defs = LCCommandRouter.get_all_command_definitions()
	output_log.append_text("Commandable Targets:\n")
	for target in defs:
		var cmd_list = []
		for c in defs[target]:
			cmd_list.append(c.name)
		output_log.append_text("  [b]%s[/b]: %s\n" % [target, ", ".join(cmd_list)])
