extends LCControllerUI

# target is inherited from LCControllerUI (typed as LCRoverController)

@onready var speed_label = get_node_or_null("SpeedLabel")
@onready var steering_label = get_node_or_null("SteeringLabel")
@onready var motor_label = get_node_or_null("MotorLabel")
@onready var camera_label = get_node_or_null("CurrentCamera")
@onready var drive_mode_select = get_node_or_null("DriveModeSelect")
@onready var wheel_controls_panel = get_node_or_null("WheelControlsPanel")

# Wheel control sliders
@onready var fl_motor_slider = get_node_or_null("WheelControlsPanel/FrontLeftPanel/FLMotor")
@onready var fr_motor_slider = get_node_or_null("WheelControlsPanel/FrontRightPanel/FRMotor")
@onready var bl_motor_slider = get_node_or_null("WheelControlsPanel/BackLeftPanel/BLMotor")
@onready var br_motor_slider = get_node_or_null("WheelControlsPanel/BackRightPanel/BRMotor")

# UI update throttling
var update_timer := 0.0
const UPDATE_INTERVAL := 0.1  # 10 fps instead of 60

func _ready():
	# Connect to avatar's camera system if available
	var avatar = _find_avatar()
	if avatar:
		# Update camera label when cameras change
		call_deferred("_update_camera_label")

func _find_avatar() -> Node:
	"""Find the avatar in the scene tree"""
	var root = get_tree().root
	var avatars = root.get_tree().get_nodes_in_group("avatar")
	if avatars.size() > 0:
		return avatars[0]
	return null

# Override base class hook to connect signals when target is set
func _on_target_set():
	if target is LCRoverController:
		target.speed_changed.connect(_on_speed_changed)
		target.steering_changed.connect(_on_steering_changed)
		target.motor_state_changed.connect(_on_motor_changed)
		
		# Initialize drive mode selector
		if drive_mode_select and "drive_mode" in target:
			drive_mode_select.selected = target.drive_mode
			_update_wheel_controls_visibility()
	else:
		push_warning("RoverUI: Target is not a rover controller")

func _process(delta):
	# Throttle UI updates to reduce performance impact
	update_timer += delta
	if update_timer >= UPDATE_INTERVAL:
		update_timer = 0.0
		_update_ui_labels()
		_update_camera_label()

func _on_speed_changed(speed: float):
	# Signal received - mark for update
	pass

func _on_steering_changed(angle: float):
	# Signal received - mark for update
	pass

func _on_motor_changed(power: float):
	# Signal received - mark for update
	pass

func _on_drive_mode_selected(index: int):
	"""Handle drive mode selection change"""
	if target and "drive_mode" in target:
		target.drive_mode = index
		# Enable individual control for Independent mode (index 3)
		if "enable_individual_control" in target:
			target.enable_individual_control = (index == 3)
		print("RoverUI: Drive mode changed to: ", ["Standard", "Ackermann", "Differential", "Independent"][index])
		_update_wheel_controls_visibility()

func _update_wheel_controls_visibility():
	"""Show/hide wheel controls based on drive mode"""
	if not wheel_controls_panel or not target:
		return
	
	# Show wheel controls only in Independent mode (index 3)
	var show_controls = target.drive_mode == 3
	wheel_controls_panel.visible = show_controls

# Wheel control slider callbacks
func _on_fl_motor_changed(value: float):
	if target and "set_wheel_motor" in target:
		target.set_wheel_motor("front_left", value)

func _on_fr_motor_changed(value: float):
	if target and "set_wheel_motor" in target:
		target.set_wheel_motor("front_right", value)

func _on_bl_motor_changed(value: float):
	if target and "set_wheel_motor" in target:
		target.set_wheel_motor("back_left", value)

func _on_br_motor_changed(value: float):
	if target and "set_wheel_motor" in target:
		target.set_wheel_motor("back_right", value)

func _update_ui_labels():
	"""Update UI labels with current values"""
	if not target:
		return

	# Get current values from target
	var speed = target.current_speed if "current_speed" in target else 0.0
	var steering = target.steering_input if "steering_input" in target else 0.0
	var motor = target.motor_input if "motor_input" in target else 0.0

	# Update labels (only if they exist)
	if speed_label:
		speed_label.text = "Speed: %.1f m/s" % speed
	if steering_label:
		steering_label.text = "Steering: %.2f" % steering
	if motor_label:
		motor_label.text = "Motor: %.0f%%" % (motor * 100)

func _update_camera_label():
	"""Update camera label with current camera info"""
	if not camera_label:
		return
	
	var avatar = _find_avatar()
	if not avatar:
		return
	
	if "available_cameras" in avatar and "current_camera_index" in avatar:
		var cameras = avatar.available_cameras
		var current_idx = avatar.current_camera_index
		
		if cameras.size() > 0 and current_idx < cameras.size():
			var camera_info = cameras[current_idx]
			camera_label.text = "Camera: %s (%d/%d)" % [
				camera_info.name,
				current_idx + 1,
				cameras.size()
			]
		else:
			camera_label.text = "Camera: Unknown"
	else:
		camera_label.text = "Camera: Third Person"
