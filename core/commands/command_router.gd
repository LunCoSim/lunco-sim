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
func unregister_executor(executor: LCCommandExecutor):
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
		for e in get_tree().get_nodes_in_group("CommandExecutors"):
			if e.alias == target_str or str(e.get_path()) == target_str or str(e.get_parent().get_path()) == target_str:
				executor = e
				register_executor(e)
				break
				
	if executor:
		return executor.execute(command)
	else:
		var err = "Command target not found: %s" % target_str
		push_warning(err)
		return err

## Executes a command from a raw dictionary (e.g., from JSON/HTTP).
func execute_raw(dict: Dictionary) -> Variant:
	var command = LCCommand.from_dict(dict)
	return dispatch(command)

## Returns a consolidated dictionary of all available commands across all entities.
func get_all_command_definitions() -> Dictionary:
	var dict = {}
	for key in _executors:
		var executor = _executors[key]
		# Avoid duplicate entries for same executor (registered by path and alias)
		if executor.alias != "" and key != executor.alias:
			continue
		dict[key] = executor.get_command_dictionary()
	return dict
