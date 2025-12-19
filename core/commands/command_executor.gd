class_name LCCommandExecutor
extends Node

## Node that handles command execution for its parent.
## Commands are mapped to parent methods with the 'cmd_' prefix.

signal command_executed(command: LCCommand, result: Variant)
signal command_failed(command: LCCommand, error: String)

@export var alias: String = "" ## Optional alias to address this executor (e.g., "rover1")

func _ready():
	print("DEBUG: LCCommandExecutor _ready called. Name: ", name, " Parent: ", get_parent())
	add_to_group("CommandExecutors")
	print("DEBUG: Added to CommandExecutors group. In group? ", is_in_group("CommandExecutors"))
	
	# Auto-generate alias if none provided
	if alias == "":
		var parent = get_parent()
		if parent:
			var grandparent = parent.get_parent()
			# If we are in a controller, use the vehicle/character name
			# If we are in a controller, use the vehicle/character name
			var is_controller = parent is LCController
			if not is_controller and parent.get_script():
				# Fallback check using resource path in case of cyclic dependency issues
				is_controller = "Controller" in parent.get_script().resource_path.get_file()
				
			if is_controller and grandparent:
				alias = grandparent.name
			else:
				alias = parent.name
				
	if LCCommandRouter:
		LCCommandRouter.register_executor(self)
		print("LCCommandExecutor: Registered self for parent: ", get_parent().get_path())

func _exit_tree():
	if LCCommandRouter:
		LCCommandRouter.unregister_executor(self)

## Executes a command.
func execute(command: LCCommand) -> Variant:
	var method_name = "cmd_" + command.name.to_lower()
	var parent = get_parent()
	
	if not parent:
		var err = "Executor has no parent"
		command_failed.emit(command, err)
		return err
		
	if not parent.has_method(method_name):
		var err = "Parent %s does not implement command method: %s" % [parent.name, method_name]
		command_failed.emit(command, err)
		return err
		
	# Execute using reflection
	var result = parent.call(method_name, command.arguments)
	command_executed.emit(command, result)
	return result

## Returns a list of available commands based on methods prefixed with 'cmd_'.
func get_command_dictionary() -> Array:
	var commands = []
	var parent = get_parent()
	if not parent:
		return []
		
	for method in parent.get_method_list():
		if method.name.begins_with("cmd_"):
			var cmd_name = method.name.substr(4).to_upper()
			commands.append({
				"name": cmd_name,
				"arguments": _get_method_params(method)
			})
	return commands

func _get_method_params(method: Dictionary) -> Array:
	var params = []
	for arg in method.args:
		params.append({
			"name": arg.name,
			"type": type_string(arg.type)
		})
	return params
