extends Control

# Get access to UI elements
@onready var record_button = $VBoxContainer/HBoxContainer/RecordButton
@onready var stop_button = $VBoxContainer/HBoxContainer/StopButton
@onready var replay_button = $VBoxContainer/HBoxContainer/ReplayButton
@onready var save_button = $VBoxContainer/HBoxContainer/SaveButton
@onready var load_button = $VBoxContainer/HBoxContainer/LoadButton
@onready var refresh_button = $VBoxContainer/HBoxContainer/RefreshButton
@onready var restart_button = $VBoxContainer/HBoxContainer/RestartButton
@onready var movie_mode_button = $VBoxContainer/HBoxContainer/MovieModeButton
@onready var speed_slider = $VBoxContainer/HBoxContainer/SpeedSlider
@onready var speed_label = $VBoxContainer/HBoxContainer/SpeedLabel
@onready var back_button = $VBoxContainer/HBoxContainer/BackButton
@onready var status_label = $VBoxContainer/StatusLabel
@onready var recordings_list = $VBoxContainer/HBoxContainer2/RecordingsList
@onready var save_dialog = $SaveDialog
@onready var load_dialog = $LoadDialog

# Default save/load paths
var default_save_path = "user://recordings/"
var last_loaded_path = ""
var current_recordings = []
var movie_mode = false

func _ready():
	prints("Replay scene ready, initializing UI...")
	
	# Ensure recordings directory exists
	ensure_recordings_directory_exists()
	
	# Connect button signals
	record_button.pressed.connect(_on_record_button_pressed)
	stop_button.pressed.connect(_on_stop_button_pressed)
	replay_button.pressed.connect(_on_replay_button_pressed)
	save_button.pressed.connect(_on_save_button_pressed)
	load_button.pressed.connect(_on_load_button_pressed)
	refresh_button.pressed.connect(_on_refresh_button_pressed)
	back_button.pressed.connect(_on_back_button_pressed)
	
	# Connect new buttons
	if restart_button:
		restart_button.pressed.connect(_on_restart_button_pressed)
	
	if movie_mode_button:
		movie_mode_button.pressed.connect(_on_movie_mode_button_pressed)
	
	# Connect list signals
	recordings_list.item_activated.connect(_on_recordings_list_item_activated)
	recordings_list.item_selected.connect(_on_recordings_list_item_selected)
	
	# Connect slider value changed signal
	speed_slider.value_changed.connect(_on_speed_slider_value_changed)
	
	# Connect file dialog signals
	save_dialog.file_selected.connect(_on_save_dialog_file_selected)
	load_dialog.file_selected.connect(_on_load_dialog_file_selected)
	
	# Connect ReplayManager signals
	ReplayManager.recording_started.connect(_on_recording_started)
	ReplayManager.recording_stopped.connect(_on_recording_stopped)
	ReplayManager.replay_started.connect(_on_replay_started)
	ReplayManager.replay_completed.connect(_on_replay_completed)
	ReplayManager.replay_stopped.connect(_on_replay_stopped)
	
	# Set initial UI state
	stop_button.disabled = true
	save_button.disabled = true
	replay_button.disabled = true
	if restart_button:
		restart_button.disabled = true
	if movie_mode_button:
		movie_mode_button.disabled = true
	
	# Check if there are any existing recordings
	check_for_recordings()

func ensure_recordings_directory_exists():
	var recordings_dir = OS.get_user_data_dir() + "/recordings/"
	prints("Checking if recordings directory exists:", recordings_dir)
	
	var dir = DirAccess.open("user://")
	if dir:
		if not dir.dir_exists("recordings"):
			prints("Creating recordings directory...")
			dir.make_dir("recordings")
			prints("Recordings directory created")
		else:
			prints("Recordings directory already exists")
	else:
		prints("Could not access user data directory!")
		
	# Verify it exists and is accessible now
	var recordings_dir_access = DirAccess.open("user://recordings/")
	if recordings_dir_access:
		prints("Successfully verified recordings directory access")
	else:
		printerr("Failed to access recordings directory even after creation attempt!")

func check_for_recordings():
	var dir = DirAccess.open(default_save_path)
	if dir:
		var has_recordings = false
		current_recordings.clear()
		recordings_list.clear()
		
		# First, check for and remove empty files
		clean_empty_recordings()
		
		dir.list_dir_begin()
		var file_name = dir.get_next()
		while file_name != "":
			if file_name.ends_with(".replay") and not dir.current_is_dir():
				has_recordings = true
				current_recordings.append(file_name)
				recordings_list.add_item(file_name)
			file_name = dir.get_next()
		
		if not has_recordings:
			update_status("No recordings found. Create one by pressing 'Start Recording'")
			replay_button.disabled = true
			load_button.disabled = true
		else:
			update_status("Found " + str(current_recordings.size()) + " recordings")
			load_button.disabled = false
			
			# Select first recording by default if we haven't loaded one yet
			if last_loaded_path == "" and recordings_list.item_count > 0:
				recordings_list.select(0)
	else:
		update_status("Could not access recordings directory")
		load_button.disabled = true

func clean_empty_recordings():
	var dir = DirAccess.open(default_save_path)
	if dir:
		dir.list_dir_begin()
		var file_name = dir.get_next()
		var removed_count = 0
		
		while file_name != "":
			if file_name.ends_with(".replay") and not dir.current_is_dir():
				var full_path = default_save_path + file_name
				var file = FileAccess.open(full_path, FileAccess.READ)
				if file:
					var content = file.get_as_text()
					file.close()
					
					# Check if the file is empty or just contains empty JSON
					if content.is_empty() or content.strip_edges() == "{}" or content.strip_edges() == "[]":
						if dir.remove(file_name) == OK:
							prints("Removed empty recording file:", file_name)
							removed_count += 1
				else:
					# If we can't open the file, it might be corrupted
					if dir.remove(file_name) == OK:
						prints("Removed corrupted recording file:", file_name)
						removed_count += 1
			
			file_name = dir.get_next()
		
		if removed_count > 0:
			prints("Cleaned up", removed_count, "empty/corrupted recording files")
	else:
		prints("Could not access recordings directory for cleanup")

func _on_recordings_list_item_activated(index):
	if index >= 0 and index < current_recordings.size():
		var recording_name = current_recordings[index]
		var full_path = OS.get_user_data_dir() + "/recordings/" + recording_name
		load_recording(full_path)

func _on_recordings_list_item_selected(index):
	if index >= 0 and index < current_recordings.size():
		# Enable replay button when a recording is selected
		replay_button.disabled = false

func load_recording(path):
	if ReplayManager.load_recording(path):
		last_loaded_path = path
		update_status("Recording loaded from: " + path.get_file())
		replay_button.disabled = false
		save_button.disabled = true
		return true
	else:
		update_status("Failed to load recording: " + path)
		prints("Failed to load recording from path:", path)
		
		# Try debugging what's happening
		if not FileAccess.file_exists(path):
			prints("File does not exist at path:", path)
		else:
			prints("File exists but could not be loaded, may be invalid format")
		return false

func _on_record_button_pressed():
	if ReplayManager.start_recording():
		update_status("Recording started")
		record_button.disabled = true
		replay_button.disabled = true
		load_button.disabled = true
		stop_button.disabled = false
		save_button.disabled = true

func _on_stop_button_pressed():
	prints("Stop button pressed, current state:", ReplayManager.current_state)
	
	if ReplayManager.current_state == ReplayManager.ReplayState.RECORDING:
		prints("Stopping recording...")
		if ReplayManager.stop_recording():
			update_status("Recording stopped")
			record_button.disabled = false
			replay_button.disabled = false
			load_button.disabled = false
			stop_button.disabled = true
			save_button.disabled = false
	elif ReplayManager.current_state == ReplayManager.ReplayState.REPLAYING:
		prints("Stopping replay...")
		if ReplayManager.stop_replay():
			update_status("Replay stopped")
			record_button.disabled = false
			replay_button.disabled = false
			load_button.disabled = false
			stop_button.disabled = true

func _on_replay_button_pressed():
	var speed = speed_slider.value
	
	prints("DEBUG: Trying to start replay - Selected items:", recordings_list.get_selected_items())
	
	# If a recording is selected in the list, load it first
	var selected_idx = recordings_list.get_selected_items()
	if selected_idx.size() > 0:
		var idx = selected_idx[0]
		prints("DEBUG: Selected recording index:", idx)
		if idx >= 0 and idx < current_recordings.size():
			var recording_name = current_recordings[idx]
			var full_path = OS.get_user_data_dir() + "/recordings/" + recording_name
			prints("DEBUG: Trying to load recording from path:", full_path)
			if last_loaded_path != full_path:
				if not load_recording(full_path):
					prints("DEBUG: Failed to load recording")
					return
				else:
					prints("DEBUG: Successfully loaded recording")
			else:
				prints("DEBUG: Recording already loaded")
		else:
			prints("DEBUG: Invalid selection index")
	else:
		prints("DEBUG: No recording selected")
	
	prints("DEBUG: Starting replay with ReplayManager.recorded_actions.size():", ReplayManager.recorded_actions.size())
	if ReplayManager.start_replay(speed):
		update_status("Replay started (Speed: " + str(speed) + "x)")
		record_button.disabled = true
		replay_button.disabled = true
		load_button.disabled = true
		stop_button.disabled = false
		if restart_button:
			restart_button.disabled = false
		if movie_mode_button:
			movie_mode_button.disabled = true
		prints("DEBUG: Replay started successfully")
	else:
		update_status("Failed to start replay")
		prints("DEBUG: Failed to start replay")

func _on_save_button_pressed():
	var timestamp = Time.get_datetime_string_from_system().replace(":", "-").replace(" ", "_")
	var default_filename = "recording_" + timestamp + ".replay"
	
	# Make sure we navigate to the recordings folder
	var recordings_dir = OS.get_user_data_dir() + "/recordings/"
	save_dialog.current_dir = recordings_dir
	save_dialog.current_path = recordings_dir + default_filename
	save_dialog.popup_centered()

func _on_load_button_pressed():
	# Make sure we navigate to the recordings folder
	var recordings_dir = OS.get_user_data_dir() + "/recordings/"
	load_dialog.current_dir = recordings_dir
	load_dialog.popup_centered()

func _on_refresh_button_pressed():
	update_status("Refreshing recordings list...")
	check_for_recordings()
	
	# If the ReplayShortcut exists, use it to list recordings
	if has_node("/root/ReplayShortcut"):
		var recordings = get_node("/root/ReplayShortcut").list_recordings()
		if recordings.size() > 0:
			update_status("Found " + str(recordings.size()) + " recordings")
			load_button.disabled = false
		else:
			update_status("No recordings found")
			load_button.disabled = true

func _on_back_button_pressed():
	# First ensure we've stopped any recording or replay
	if ReplayManager.current_state == ReplayManager.ReplayState.RECORDING:
		ReplayManager.stop_recording()
	elif ReplayManager.current_state == ReplayManager.ReplayState.REPLAYING:
		ReplayManager.stop_replay()
	
	# Go back to the previous scene
	StateManager.back_from_replay()

func _on_speed_slider_value_changed(value):
	speed_label.text = "Speed: " + str(snappedf(value, 0.1)) + "x"
	
	# If we're currently replaying, update the replay speed
	if ReplayManager.current_state == ReplayManager.ReplayState.REPLAYING:
		ReplayManager.replay_time_scale = value
		update_status("Replay speed changed to " + str(snappedf(value, 0.1)) + "x")

func _on_save_dialog_file_selected(path):
	# Ensure path ends with .replay
	if not path.ends_with(".replay"):
		path += ".replay"
	
	# Ensure directory exists
	var dir = path.get_base_dir()
	if not DirAccess.dir_exists_absolute(dir):
		var dir_access = DirAccess.open("user://")
		if dir_access and not dir_access.dir_exists("recordings"):
			dir_access.make_dir("recordings")
	
	if ReplayManager.save_recording(path):
		update_status("Recording saved to: " + path.get_file())
		save_button.disabled = true
		check_for_recordings()
	else:
		update_status("Failed to save recording to: " + path)

func _on_load_dialog_file_selected(path):
	prints("Selected recording file:", path)
	
	# If the path doesn't exist, try prepending the user data directory
	if not FileAccess.file_exists(path):
		var user_dir = OS.get_user_data_dir()
		var adjusted_path = user_dir + "/recordings/" + path.get_file()
		prints("File not found, trying adjusted path:", adjusted_path)
		
		if FileAccess.file_exists(adjusted_path):
			path = adjusted_path
			prints("Found file at adjusted path")
	
	load_recording(path)

func _on_recording_started():
	update_status("Recording started")

func _on_recording_stopped():
	update_status("Recording stopped")
	record_button.disabled = false
	replay_button.disabled = false
	load_button.disabled = false
	stop_button.disabled = true
	save_button.disabled = false
	check_for_recordings()

func _on_replay_started():
	prints("Replay started callback received")
	update_status("Replay started")
	
	# Update UI
	record_button.disabled = true
	replay_button.disabled = true
	load_button.disabled = true
	stop_button.disabled = false
	if restart_button:
		restart_button.disabled = false
	if movie_mode_button:
		movie_mode_button.disabled = true

func _on_replay_completed():
	prints("Replay completed callback received")
	update_status("Replay completed")
	
	# Update UI
	record_button.disabled = false
	replay_button.disabled = false
	load_button.disabled = false
	stop_button.disabled = true
	if restart_button:
		restart_button.disabled = false
	if movie_mode_button:
		movie_mode_button.disabled = false
	
	# In movie mode, we might want to automatically restart
	if movie_mode and has_node("MoviePanel"):
		# Wait a short time before restarting
		await get_tree().create_timer(1.0).timeout
		_on_restart_button_pressed()

func _on_replay_stopped():
	prints("Replay stopped callback received")
	update_status("Replay stopped")
	
	# Update UI
	record_button.disabled = false
	replay_button.disabled = false
	load_button.disabled = false
	stop_button.disabled = true
	if restart_button:
		restart_button.disabled = false
	if movie_mode_button:
		movie_mode_button.disabled = false

func _on_restart_button_pressed():
	prints("Restart button pressed - restarting replay from beginning")
	
	if last_loaded_path.is_empty():
		# If no recording is loaded, try to load the selected one
		var selected_idx = recordings_list.get_selected_items()
		if selected_idx.size() > 0:
			var idx = selected_idx[0]
			if idx >= 0 and idx < current_recordings.size():
				var recording_name = current_recordings[idx]
				last_loaded_path = OS.get_user_data_dir() + "/recordings/" + recording_name
	
	if not last_loaded_path.is_empty():
		# Stop the current replay if it's running
		if ReplayManager.current_state == ReplayManager.ReplayState.REPLAYING:
			ReplayManager.stop_replay()
		
		# Reload the recording and start again
		if ReplayManager.load_recording(last_loaded_path):
			var speed = speed_slider.value
			if ReplayManager.start_replay(speed):
				update_status("Replay restarted from beginning (Speed: " + str(speed) + "x)")
				record_button.disabled = true
				replay_button.disabled = true
				load_button.disabled = true
				stop_button.disabled = false
				if restart_button:
					restart_button.disabled = false
				if movie_mode_button:
					movie_mode_button.disabled = true
				prints("Replay restarted successfully")
			else:
				update_status("Failed to restart replay")
		else:
			update_status("Failed to reload recording")
	else:
		update_status("No recording loaded to restart")

func _on_movie_mode_button_pressed():
	movie_mode = !movie_mode
	if movie_mode:
		update_status("Movie maker mode activated")
		movie_mode_button.text = "Exit Movie Mode"
		
		# Set the viewport to capture frames if possible
		# This depends on your project setup, but here's a general approach
		if get_viewport().has_method("set_clear_mode"):
			get_viewport().set_clear_mode(SubViewport.CLEAR_MODE_ALWAYS)
		
		# Setup for video output
		setup_movie_maker_mode()
	else:
		update_status("Movie maker mode deactivated")
		movie_mode_button.text = "Movie Maker Mode"
		
		# Clean up movie maker mode
		cleanup_movie_maker_mode()

func setup_movie_maker_mode():
	# Hide UI elements for clean recording
	$VBoxContainer/HBoxContainer.visible = false
	$VBoxContainer/StatusLabel.visible = false
	$VBoxContainer/HBoxContainer2.visible = false
	
	# Create a small control panel for movie mode
	var movie_panel = VBoxContainer.new()
	movie_panel.name = "MoviePanel"
	movie_panel.anchor_left = 1.0
	movie_panel.anchor_top = 0.0
	movie_panel.anchor_right = 1.0
	movie_panel.anchor_bottom = 0.0
	movie_panel.offset_left = -200
	movie_panel.offset_top = 10
	movie_panel.offset_right = -10
	add_child(movie_panel)
	
	# Add restart, stop and exit buttons
	var restart_btn = Button.new()
	restart_btn.text = "Restart"
	restart_btn.pressed.connect(_on_restart_button_pressed)
	movie_panel.add_child(restart_btn)
	
	var stop_btn = Button.new()
	stop_btn.text = "Stop"
	stop_btn.pressed.connect(_on_stop_button_pressed)
	movie_panel.add_child(stop_btn)
	
	var exit_btn = Button.new()
	exit_btn.text = "Exit Movie Mode"
	exit_btn.pressed.connect(_on_movie_mode_button_pressed)
	movie_panel.add_child(exit_btn)
	
	# Auto-start replay from beginning if we have a loaded recording
	_on_restart_button_pressed()

func cleanup_movie_maker_mode():
	# Show UI elements again
	$VBoxContainer/HBoxContainer.visible = true
	$VBoxContainer/StatusLabel.visible = true
	$VBoxContainer/HBoxContainer2.visible = true
	
	# Remove movie control panel
	if has_node("MoviePanel"):
		get_node("MoviePanel").queue_free()

func update_status(message):
	status_label.text = "Status: " + message 

func get_normalized_path(path):
	# Standardize paths to ensure compatibility
	if not path.begins_with("user://") and not path.begins_with("res://") and not path.begins_with("/"):
		# If it's a relative path, prepend user://recordings/
		return "user://recordings/" + path.get_file()
	return path 