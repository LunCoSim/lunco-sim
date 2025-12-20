extends Window

# Reference UIHelper singleton
@onready var ui_helper = get_node("/root/UIHelper")

var target
var update_timer = 0

func _ready():
	# Apply consistent UI styling using our helper
	ui_helper.setup_window(self)

	var panel = $PanelContainer
	var margin = $PanelContainer/MarginContainer
	var vbox = $PanelContainer/MarginContainer/VBoxContainer

	# Apply consistent styling to containers
	ui_helper.setup_panel(panel)
	ui_helper.setup_containers(vbox, margin)

	set_process(true)

func _process(delta):
	update_timer += delta
	# Update stats every 0.2 seconds
	if update_timer >= 0.2 and target and is_instance_valid(target):
		update_timer = 0
		update_stats()

func set_target(_target):
	target = _target

func update_stats():
	if target and is_instance_valid(target):
		# Get character velocity and position
		var velocity = target.get_linear_velocity() if target.has_method("get_linear_velocity") else Vector3.ZERO
		var position = target.get_position() if target.has_method("get_position") else Vector3.ZERO
		var speed = velocity.length()
		
		# Update UI elements
		%SpeedValue.text = "%.2f m/s" % speed
		%PosValue.text = "%.1f, %.1f, %.1f" % [position.x, position.y, position.z]
		%GravValue.text = "1.625 m/s²"  # Moon gravity constant

func _on_close_requested():
	hide()

func _on_reset_button_pressed():
	if target and target.has_method("reset_camera"):
		target.reset_camera()

func _on_release_control_button_pressed():
	if target and target.has_method("release_control"):
		target.release_control()
	else:
		# Fallback to parent
		var parent = get_parent()
		while parent:
			if parent.has_method("release_control"):
				parent.release_control()
				break
			parent = parent.get_parent()
