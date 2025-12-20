extends Node

# Signals
signal recording_started
signal recording_stopped
signal replay_started
signal replay_completed
signal replay_stopped

enum ReplayState {
	IDLE,
	RECORDING,
	REPLAYING
}

var current_state = ReplayState.IDLE
var recorded_actions = []
var current_frame = 0
var replay_timer = 0.0
var record_start_time = 0.0
var replay_time_scale = 1.0  # Can be adjusted for slow-motion or fast-forward
var recording_metadata = {}

# Configuration
var should_record_mouse_motion = true  # Set to false if you want to ignore mouse motion events

func _ready():
	# Add the singleton to the autoload list in Project Settings
	pass

func _process(delta):
	if current_state == ReplayState.REPLAYING:
		replay_timer += delta * replay_time_scale
		_process_replay_frame()

		# Add a check for completion - if we've processed all frames, emit completion
		if current_frame >= recorded_actions.size() and recorded_actions.size() > 0:
			prints("Replay completed naturally after reaching the end")
			stop_replay()
			replay_completed.emit()

func start_recording():
	if current_state != ReplayState.IDLE:
		return false

	print("Starting recording...")
	recorded_actions.clear()
	current_frame = 0
	record_start_time = Time.get_ticks_msec() / 1000.0
	recording_metadata = {
		"version": "1.0",
		"date": Time.get_datetime_string_from_system(),
		"engine_version": Engine.get_version_info(),
		"project_name": ProjectSettings.get_setting("application/config/name"),
		"project_version": ProjectSettings.get_setting("application/config/version")
	}

	current_state = ReplayState.RECORDING
	recording_started.emit()

	# Start intercepting input events - use window_input instead of gui_input
	get_tree().get_root().set_input_as_handled()
	get_tree().get_root().window_input.connect(_on_input_event)

	return true

func stop_recording():
	if current_state != ReplayState.RECORDING:
		return false

	print("Stopping recording...")
	current_state = ReplayState.IDLE

	# Restore normal input processing - disconnect window_input
	if get_tree().get_root().window_input.is_connected(_on_input_event):
		get_tree().get_root().window_input.disconnect(_on_input_event)

	recording_stopped.emit()
	return true

func start_replay(time_scale = 1.0):
	if current_state != ReplayState.IDLE:
		push_error("Cannot start replay: not in IDLE state")
		return false

	if recorded_actions.is_empty():
		push_error("Cannot start replay: no recorded actions available")
		return false

	prints("Starting replay with", recorded_actions.size(), "actions, time scale:", time_scale)

	# Reset replay state
	current_frame = 0
	replay_timer = 0.0
	replay_time_scale = time_scale

	# Set state and emit signal
	current_state = ReplayState.REPLAYING
	replay_started.emit()

	return true

func stop_replay():
	if current_state != ReplayState.REPLAYING:
		push_error("Cannot stop replay: not in REPLAYING state")
		return false

	prints("Stopping replay at frame", current_frame, "of", recorded_actions.size())

	# Reset state
	current_state = ReplayState.IDLE

	# Emit signal
	replay_stopped.emit()
	return true

func _on_input_event(event):
	if current_state != ReplayState.RECORDING:
		return

	# Skip mouse motion events if configured to do so
	if not should_record_mouse_motion and event is InputEventMouseMotion:
		return

	# Record the event with timestamp
	var timestamp = Time.get_ticks_msec() / 1000.0 - record_start_time
	var serialized_event = _serialize_input_event(event)
	if serialized_event:
		recorded_actions.append({
			"timestamp": timestamp,
			"event": serialized_event
		})

		current_frame += 1

func _process_replay_frame():
	# Find all events that should be processed at the current replay time
	var events_to_process = []

	for i in range(current_frame, recorded_actions.size()):
		var action = recorded_actions[i]
		if action.has("timestamp") and action.timestamp <= replay_timer:
			events_to_process.append(action)
			current_frame = i + 1
		else:
			break

	# Process the events
	for action in events_to_process:
		if not action.has("event") or action.event == null:
			prints("Skipping invalid event at frame", current_frame - 1)
			continue

		var event = _deserialize_input_event(action.event)
		if event:
			# Debug output to help diagnose issues
			prints("Processing event:", event.get_class(), "at time:", action.timestamp)

			# Simulate the input event
			Input.parse_input_event(event)
		else:
			prints("Failed to deserialize event at frame", current_frame - 1)

	# We no longer check for completion here since we do it in _process

func _serialize_input_event(event):
	var data = {}

	# Common properties for all input events
	data["class"] = event.get_class()
	data["device"] = event.device

	if event is InputEventKey:
		data["scancode"] = event.keycode
		data["physical_scancode"] = event.physical_keycode
		data["unicode"] = event.unicode
		data["echo"] = event.echo
		data["pressed"] = event.pressed
		data["alt"] = event.alt_pressed
		data["shift"] = event.shift_pressed
		data["ctrl"] = event.ctrl_pressed
		data["meta"] = event.meta_pressed

	elif event is InputEventMouseButton:
		data["position"] = {"x": event.position.x, "y": event.position.y}
		data["global_position"] = {"x": event.global_position.x, "y": event.global_position.y}
		data["button_index"] = event.button_index
		data["pressed"] = event.pressed
		data["factor"] = event.factor
		data["double_click"] = event.double_click

	elif event is InputEventMouseMotion:
		data["position"] = {"x": event.position.x, "y": event.position.y}
		data["global_position"] = {"x": event.global_position.x, "y": event.global_position.y}
		data["velocity"] = {"x": event.velocity.x, "y": event.velocity.y}
		data["relative"] = {"x": event.relative.x, "y": event.relative.y}

	elif event is InputEventJoypadButton:
		data["button_index"] = event.button_index
		data["pressed"] = event.pressed
		data["pressure"] = event.pressure

	elif event is InputEventJoypadMotion:
		data["axis"] = event.axis
		data["axis_value"] = event.axis_value

	else:
		# Unsupported event type
		return null

	return data

func _deserialize_input_event(data):
	if not data or not data.has("class"):
		return null

	var event = null

	match data.class:
		"InputEventKey":
			event = InputEventKey.new()
			if data.has("scancode"): event.keycode = data.scancode
			if data.has("physical_scancode"): event.physical_keycode = data.physical_scancode
			if data.has("unicode"): event.unicode = data.unicode
			if data.has("echo"): event.echo = data.echo
			if data.has("pressed"): event.pressed = data.pressed
			if data.has("alt"): event.alt_pressed = data.alt
			if data.has("shift"): event.shift_pressed = data.shift
			if data.has("ctrl"): event.ctrl_pressed = data.ctrl
			if data.has("meta"): event.meta_pressed = data.meta

		"InputEventMouseButton":
			event = InputEventMouseButton.new()
			if data.has("position"): event.position = Vector2(data.position.x, data.position.y)
			if data.has("global_position"): event.global_position = Vector2(data.global_position.x, data.global_position.y)
			if data.has("button_index"): event.button_index = data.button_index
			if data.has("pressed"): event.pressed = data.pressed
			if data.has("factor"): event.factor = data.factor
			if data.has("double_click"): event.double_click = data.double_click

		"InputEventMouseMotion":
			event = InputEventMouseMotion.new()
			if data.has("position"): event.position = Vector2(data.position.x, data.position.y)
			if data.has("global_position"): event.global_position = Vector2(data.global_position.x, data.global_position.y)
			if data.has("velocity"): event.velocity = Vector2(data.velocity.x, data.velocity.y)
			if data.has("relative"): event.relative = Vector2(data.relative.x, data.relative.y)

		"InputEventJoypadButton":
			event = InputEventJoypadButton.new()
			if data.has("button_index"): event.button_index = data.button_index
			if data.has("pressed"): event.pressed = data.pressed
			if data.has("pressure"): event.pressure = data.pressure

		"InputEventJoypadMotion":
			event = InputEventJoypadMotion.new()
			if data.has("axis"): event.axis = data.axis
			if data.has("axis_value"): event.axis_value = data.axis_value

		_:
			# Unsupported event type
			prints("Unsupported event type:", data.class)
			return null

	# Set common properties
	if event and data.has("device"):
		event.device = data.device

	return event

func save_recording(file_path: String) -> bool:
	if recorded_actions.is_empty():
		push_error("No recorded actions to save")
		return false

	# Check if there are actual events in the recording
	var has_real_events = false
	for action in recorded_actions:
		if action.has("event") and action.event != null:
			has_real_events = true
			break

	if not has_real_events:
		push_error("No valid events in the recording")
		return false

	# Ensure the file path is correct
	if not file_path.begins_with("user://") and not file_path.begins_with("res://") and not file_path.begins_with("/"):
		file_path = "user://recordings/" + file_path.get_file()

	# Ensure directory exists
	var dir_path = file_path.get_base_dir()
	if not DirAccess.dir_exists_absolute(dir_path):
		DirAccess.make_dir_recursive_absolute(dir_path)

	# Save the recording to the file
	var file = FileAccess.open(file_path, FileAccess.WRITE)
	if not file:
		push_error("Failed to open file for writing: " + file_path + " Error: " + str(FileAccess.get_open_error()))
		return false

	# Convert the recording to a JSON string
	var data = {
		"inputs": recorded_actions,
		"metadata": recording_metadata,
		"version": "1.0"
	}

	var json_string = JSON.stringify(data)
	file.store_string(json_string)
	prints("Recording saved successfully to", file_path, "with", recorded_actions.size(), "events")
	return true

func load_recording(file_path: String) -> bool:
	# Ensure the file path is correct
	if not file_path.begins_with("user://") and not file_path.begins_with("res://") and not file_path.begins_with("/"):
		file_path = "user://recordings/" + file_path.get_file()

	# Load the recording from the file
	if not FileAccess.file_exists(file_path):
		push_error("Recording file does not exist: " + file_path)
		return false

	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file for reading: " + file_path + " Error: " + str(FileAccess.get_open_error()))
		return false

	var json_string = file.get_as_text()
	var json = JSON.new()
	var error = json.parse(json_string)

	if error != OK:
		push_error("Failed to parse recording JSON: " + json.get_error_message() + " at line " + str(json.get_error_line()))
		return false

	var data = json.get_data()

	# Handle both old and new format
	if data.has("inputs"):
		recorded_actions = data["inputs"]
	elif data.has("actions"):
		recorded_actions = data["actions"]
	else:
		push_error("Recording file has invalid format, no 'inputs' or 'actions' field: " + file_path)
		return false

	# Get metadata if available
	if data.has("metadata"):
		recording_metadata = data["metadata"]

	prints("Recording loaded successfully from", file_path, "with", recorded_actions.size(), "entries")
	return true
