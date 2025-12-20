class_name lsStateManager
extends Node

@export var Username := ""
var previous_scene_path := ""


func _save():
	pass

func _load():
	pass

func change_scene(scene: String):
	pass
	#SceneManager.no_effect_change_scene(scene)

# Add a method to transition to the replay scene
func goto_replay_scene():
	# Ensure recordings directory exists
	var dir = DirAccess.open("user://")
	if dir and not dir.dir_exists("recordings"):
		dir.make_dir("recordings")
		prints("Created recordings directory at", OS.get_user_data_dir() + "/recordings/")

	# First try loading from the scenes directory
	var replay_scene = load("res://scenes/replay_scene.tscn")

	if not replay_scene:
		# If not found, try creating it in memory
		print("Replay scene not found at expected path, creating it in memory...")

		# Create a minimal replay scene if it doesn't exist
		var scene_root = Control.new()
		scene_root.name = "ReplayScene"

		var script = load("res://scenes/replay_scene.gd")
		if script:
			scene_root.set_script(script)

		var vbox = VBoxContainer.new()
		vbox.name = "VBoxContainer"
		vbox.anchor_right = 1.0
		vbox.anchor_bottom = 1.0
		scene_root.add_child(vbox)

		var hbox = HBoxContainer.new()
		hbox.name = "HBoxContainer"
		vbox.add_child(hbox)

		var record_btn = Button.new()
		record_btn.name = "RecordButton"
		record_btn.text = "Start Recording"
		hbox.add_child(record_btn)

		var stop_btn = Button.new()
		stop_btn.name = "StopButton"
		stop_btn.text = "Stop"
		hbox.add_child(stop_btn)

		var replay_btn = Button.new()
		replay_btn.name = "ReplayButton"
		replay_btn.text = "Start Replay"
		hbox.add_child(replay_btn)

		var save_btn = Button.new()
		save_btn.name = "SaveButton"
		save_btn.text = "Save Recording"
		hbox.add_child(save_btn)

		var load_btn = Button.new()
		load_btn.name = "LoadButton"
		load_btn.text = "Load Recording"
		hbox.add_child(load_btn)

		var back_btn = Button.new()
		back_btn.name = "BackButton"
		back_btn.text = "Back to Game"
		hbox.add_child(back_btn)

		var status_label = Label.new()
		status_label.name = "StatusLabel"
		status_label.text = "Status: Ready"
		vbox.add_child(status_label)

		# Add save dialog without setting root_subfolder, which is causing the error
		var save_dialog = FileDialog.new()
		save_dialog.name = "SaveDialog"
		save_dialog.title = "Save Recording"
		save_dialog.access = FileDialog.ACCESS_USERDATA
		save_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
		save_dialog.filters = PackedStringArray(["*.replay ; Replay Files"])
		scene_root.add_child(save_dialog)

		# Add load dialog without setting root_subfolder
		var load_dialog = FileDialog.new()
		load_dialog.name = "LoadDialog"
		load_dialog.title = "Load Recording"
		load_dialog.access = FileDialog.ACCESS_USERDATA
		load_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
		load_dialog.filters = PackedStringArray(["*.replay ; Replay Files"])
		scene_root.add_child(load_dialog)

		# Create a packed scene
		var packed_scene = PackedScene.new()
		packed_scene.pack(scene_root)
		replay_scene = packed_scene

	if replay_scene:
		var current_scene = get_tree().current_scene
		# Store the current scene to return to it later
		previous_scene_path = current_scene.scene_file_path

		# Transition to the replay scene
		get_tree().change_scene_to_packed(replay_scene)
	else:
		push_error("Failed to load replay scene")

# Add a method to return to the previous scene
func back_from_replay():
	if previous_scene_path:
		get_tree().change_scene_to_file(previous_scene_path)
	else:
		push_error("No previous scene stored")
