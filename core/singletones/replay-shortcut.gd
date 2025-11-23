extends Node

# This script handles keyboard shortcuts for the replay system
# It will be added as an autoload singleton

var is_recording = false
var default_save_path = "user://recordings/"

func _ready():
	# Create the recordings directory if it doesn't exist
	var dir = DirAccess.open("user://")
	if dir and not dir.dir_exists("recordings"):
		dir.make_dir("recordings")
	
	prints("ReplayShortcut initialized!", "Press Ctrl+R to toggle recording")
	list_recordings()

func _input(event):
	if Input.is_action_just_pressed("toggle_recording"):
		toggle_recording()

func toggle_recording():
	if is_recording:
		stop_recording()
	else:
		start_recording()

func start_recording():
	if ReplayManager.start_recording():
		is_recording = true
		show_notification("Recording started")
	else:
		show_notification("Failed to start recording")

func stop_recording():
	if ReplayManager.stop_recording():
		is_recording = false
		show_notification("Recording stopped")
		
		# Auto-save the recording with timestamp
		var timestamp = Time.get_datetime_string_from_system().replace(":", "-").replace(" ", "_")
		var file_path = default_save_path + "auto_recording_" + timestamp + ".replay"
		
		if ReplayManager.save_recording(file_path):
			show_notification("Recording saved to: " + file_path)
			list_recordings()
		else:
			show_notification("Failed to save recording")
	else:
		show_notification("Failed to stop recording")

func show_notification(message: String):
	prints("[Replay]", message)
	
	# Add a notification manager reference if one exists
	if has_node("/root/NotificationManager"):
		var notification_manager = get_node("/root/NotificationManager")
		notification_manager.show_notification(message)

func list_recordings():
	prints("Listing recordings from directory:", OS.get_user_data_dir() + "/recordings/")
	var dir = DirAccess.open("user://recordings/")
	var recordings = []
	
	if dir:
		prints("Successfully opened recordings directory")
		dir.list_dir_begin()
		var file_name = dir.get_next()
		while file_name != "":
			prints("Found file:", file_name)
			if file_name.ends_with(".replay") and not dir.current_is_dir():
				var recording_name = file_name
				recordings.append(recording_name)
				prints("Found recording:", recording_name)
			file_name = dir.get_next()
		
		if recordings.size() > 0:
			prints("Found", recordings.size(), "recordings in", OS.get_user_data_dir() + "/recordings/")
		else:
			prints("No recordings found in", OS.get_user_data_dir() + "/recordings/")
	else:
		prints("Failed to open recordings directory:", OS.get_user_data_dir() + "/recordings/")
		
		# Try to create the directory if it doesn't exist
		var base_dir = DirAccess.open("user://")
		if base_dir:
			if not base_dir.dir_exists("recordings"):
				prints("Creating recordings directory...")
				if base_dir.make_dir("recordings") == OK:
					prints("Successfully created recordings directory")
				else:
					prints("Failed to create recordings directory")
			else:
				prints("Recordings directory exists but couldn't be opened")
	
	return recordings 
