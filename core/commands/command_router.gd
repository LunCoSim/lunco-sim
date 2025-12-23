extends Node

## Central entry point for routing commands.
## Maintains a registry of active executors and handles dispatching.

var _executors: Dictionary = {} # alias_or_path -> LCCommandExecutor

## Registers an executor with the router.
func register_executor(executor: Node):
	if "alias" in executor and executor.alias != "":
		_executors[executor.alias] = executor
	
	_executors[str(executor.get_path())] = executor
	# Also register by parent path for convenience
	_executors[str(executor.get_parent().get_path())] = executor

## Unregisters an executor.
func unregister_executor(executor: Node):
	if executor.alias != "" and _executors.get(executor.alias) == executor:
		_executors.erase(executor.alias)
	
	var path = str(executor.get_path())
	if _executors.get(path) == executor:
		_executors.erase(path)
		
	var parent_path = str(executor.get_parent().get_path())
	if _executors.get(parent_path) == executor:
		_executors.erase(parent_path)

## Dispatches a command to the appropriate executor.
func dispatch(command: LCCommand) -> Variant:
	var target_str = str(command.target_path)
	var executor = _executors.get(target_str)
	
	if not executor:
		# Fallback: try to find it in group if not registered (e.g., if it was just spawned)
		var group_nodes = get_tree().get_nodes_in_group("CommandExecutors")
		# print("Debug: Dispatching to '%s'. Executors registered: %d. Group size: %d" % [target_str, _executors.size(), group_nodes.size()])
		
		for e in group_nodes:
			var parent_path = str(e.get_parent().get_path())
			# print("Debug: Check fallback: %s vs %s (Parent)" % [target_str, parent_path])
			
			if e.alias == target_str or str(e.get_path()) == target_str or parent_path == target_str:
				executor = e
				register_executor(e)
				print("Debug: Found executor via fallback for %s" % target_str)
				break
				
	if executor:
		return executor.execute(command)
	else:
		var err = "Command target not found: %s" % target_str
		push_warning(err)
		
		# DIAGNOSTIC DUMP
		var group = get_tree().get_nodes_in_group("CommandExecutors")
		push_warning("--- DIAGNOSTICS ---")
		push_warning("Group 'CommandExecutors' count: %d" % group.size())
		for e in group:
			push_warning("  Executor: %s | Parent: %s | Alias: %s" % [e.get_path(), e.get_parent().get_path(), e.alias])
		push_warning("-------------------")
		
		return err
		
## Executes a command from a raw dictionary (e.g., from JSON/HTTP).
func execute_raw(dict: Dictionary) -> Variant:
	var command = LCCommand.from_dict(dict)
	return dispatch(command)

## Returns a consolidated dictionary of all available commands across all entities.
func get_all_command_definitions() -> Dictionary:
	# First, ensure all current executors in group are registered
	var group_nodes = get_tree().get_nodes_in_group("CommandExecutors")
	
	# Fallback 1: search controllers group
	if group_nodes.is_empty():
		for controller in get_tree().get_nodes_in_group("controllers"):
			for child in controller.get_children():
				if child is LCCommandExecutor:
					group_nodes.append(child)
	
	# Fallback 2: absolute search (slow but guaranteed)
	if group_nodes.is_empty():
		group_nodes = _find_executors_recursive(get_tree().root)
					
	if group_nodes.is_empty():
		print("LCCommandRouter: No executors found in group or tree search.")
	else:
		print("LCCommandRouter: Found %d executors." % group_nodes.size())
					
	for e in group_nodes:
		if not str(e.get_path()) in _executors:
			register_executor(e)
			
	var dict = {}
	for key in _executors:
		var executor = _executors[key]
		# Avoid duplicate entries for same executor (registered by path and alias)
		if executor.alias != "" and key != executor.alias:
			continue
		dict[key] = executor.get_command_dictionary()
	return dict

func _find_executors_recursive(node: Node) -> Array:
	var results = []
	if node is LCCommandExecutor:
		results.append(node)
	for child in node.get_children():
		results.append_array(_find_executors_recursive(child))
	return results
