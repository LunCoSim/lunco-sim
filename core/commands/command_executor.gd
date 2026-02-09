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
	
	# Get method info via reflection
	var method_info = _get_method_info(parent, method_name)
	if not method_info:
		var err = "Failed to get method info for: %s" % method_name
		command_failed.emit(command, err)
		return err
	
	# Convert arguments to match parameter types
	var converted_args = _convert_arguments(command.arguments, method_info.args)
	if converted_args == null:
		var err = "Failed to convert arguments for: %s" % method_name
		command_failed.emit(command, err)
		return err
	
	# Call with proper arguments - support async commands
	var result = await parent.callv(method_name, converted_args)
	command_executed.emit(command, result)
	return result

## Returns a list of available commands based on methods prefixed with 'cmd_'.
func get_command_dictionary() -> Array:
	var commands = []
	var parent = get_parent()
	if not parent:
		return []
		
	var metadata = {}
	if parent.has_method("get_command_metadata"):
		metadata = parent.get_command_metadata()
		
	for method in parent.get_method_list():
		if method.name.begins_with("cmd_"):
			var cmd_name = method.name.substr(4).to_upper()
			var cmd_info = {
				"name": cmd_name,
				"arguments": _get_method_params(method)
			}
			
			# Merge metadata if available
			if metadata.has(cmd_name):
				var cmd_meta = metadata[cmd_name]
				
				# If metadata has detailed arguments, use them instead of the reflected 'args' dict
				if cmd_meta.has("arguments"):
					var detailed_args = []
					var meta_args = cmd_meta.arguments
					if meta_args is Dictionary:
						for arg_name in meta_args:
							var arg_info = meta_args[arg_name].duplicate()
							arg_info["name"] = arg_name
							detailed_args.append(arg_info)
					elif meta_args is Array:
						for arg in meta_args:
							if arg is Dictionary:
								detailed_args.append(arg.duplicate())
							else:
								# Fallback for simple arrays
								detailed_args.append({"name": str(arg)})
					cmd_info.arguments = detailed_args
				
				# Allow metadata to override/add other fields (description, etc.)
				for key in cmd_meta:
					if key != "arguments":
						cmd_info[key] = cmd_meta[key]
						
			commands.append(cmd_info)
	return commands

func _get_method_params(method: Dictionary) -> Array:
	var params = []
	for arg in method.args:
		params.append({
			"name": arg.name,
			"type": type_string(arg.type)
		})
	return params

## Get method info for a specific method name
func _get_method_info(obj: Object, method_name: String) -> Dictionary:
	for method in obj.get_method_list():
		if method.name == method_name:
			return method
	return {}

## Convert command arguments to match method signature
func _convert_arguments(args: Variant, method_args: Array) -> Variant:
	# If args is already an Array and matches expected count, use it directly
	if args is Array:
		var args_array = args as Array
		# Pad with nulls if fewer args provided than expected
		while args_array.size() < method_args.size():
			args_array.append(null)
		return args_array
	
	# If args is a Dictionary, convert to positional array based on parameter names
	if args is Dictionary:
		var result = []
		for param in method_args:
			var param_name = param.name
			var param_type = param.type
			
			# Get value from dictionary
			var value = args.get(param_name, null)
			
			# Convert to expected type
			if value != null:
				value = _convert_to_type(value, param_type)
			
			result.append(value)
		
		return result
	
	# Single value - wrap in array
	return [args]

## Convert a value to the expected type
func _convert_to_type(value: Variant, target_type: int) -> Variant:
	# Already correct type
	if typeof(value) == target_type:
		return value
	
	# Type conversion
	match target_type:
		TYPE_BOOL:
			if value is String:
				return value.to_lower() in ["true", "1", "yes"]
			return bool(value)
		TYPE_INT:
			return int(value)
		TYPE_FLOAT:
			return float(value)
		TYPE_STRING:
			return str(value)
		TYPE_VECTOR2:
			if value is Array and value.size() >= 2:
				return Vector2(float(value[0]), float(value[1]))
			return value
		TYPE_VECTOR3:
			if value is Array and value.size() >= 3:
				return Vector3(float(value[0]), float(value[1]), float(value[2]))
			return value
		_:
			# For other types, return as-is
			return value
