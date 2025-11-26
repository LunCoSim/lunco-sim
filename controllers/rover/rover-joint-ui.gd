extends LCControllerUI

## UI for advanced rover joint control
## Shows individual wheel controls and drive mode selection

@onready var speed_label = $PanelContainer/VBox/StatusPanel/GridContainer/SpeedValue
@onready var mode_label = $PanelContainer/VBox/StatusPanel/GridContainer/ModeValue
@onready var motor_value = $PanelContainer/VBox/StatusPanel/GridContainer/MotorValue
@onready var steering_value = $PanelContainer/VBox/StatusPanel/GridContainer/SteeringValue

# UI Container references
@onready var panel_container = $PanelContainer
@onready var vbox_container = $PanelContainer/VBox
@onready var scroll_container = $PanelContainer/VBox/ScrollContainer

# Drive mode controls
@onready var mode_selector = $PanelContainer/VBox/ModePanel/VBox/HBox/ModeSelector
@onready var settings_button = $PanelContainer/VBox/ModePanel/VBox/HBox/SettingsButton
@onready var parameter_editor = $PanelContainer/VBox/ParameterEditor

# Animation
var resize_tween: Tween

# Wheel control panels
@onready var wheel_controls_container = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel
@onready var fl_motor_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontLeft/MotorSlider
@onready var fl_brake_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontLeft/BrakeSlider
@onready var fl_steering_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontLeft/SteeringSlider

@onready var fr_motor_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontRight/MotorSlider
@onready var fr_brake_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontRight/BrakeSlider
@onready var fr_steering_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontRight/SteeringSlider

@onready var bl_motor_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackLeft/MotorSlider
@onready var bl_brake_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackLeft/BrakeSlider
@onready var bl_steering_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackLeft/SteeringSlider

@onready var br_motor_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackRight/MotorSlider
@onready var br_brake_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackRight/BrakeSlider
@onready var br_steering_slider = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackRight/SteeringSlider

# Telemetry labels
@onready var fl_rpm_label = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontLeft/RPMLabel
@onready var fr_rpm_label = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/FrontRight/RPMLabel
@onready var bl_rpm_label = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackLeft/RPMLabel
@onready var br_rpm_label = $PanelContainer/VBox/ScrollContainer/WheelControlsPanel/BackRight/RPMLabel

var drive_modes = ["Standard", "Ackermann", "Differential", "Independent"]

func _ready():
	# Setup mode selector
	if mode_selector:
		for mode in drive_modes:
			mode_selector.add_item(mode)
		# Don't set selected here - wait for target to be set

func _on_target_set():
	"""Called when target controller is set"""
	if target is LCRoverJointController:
		# Connect signals
		target.speed_changed.connect(_on_speed_changed)
		target.motor_state_changed.connect(_on_motor_changed)
		target.steering_changed.connect(_on_steering_changed)
		target.wheel_control_changed.connect(_on_wheel_control_changed)
		
		# Update UI to match current mode
		if mode_selector:
			mode_selector.selected = target.drive_mode
		
		_update_mode_display()
		_update_wheel_controls_visibility()
	else:
		push_warning("RoverJointUI: Target is not a LCRoverJointController")

func _process(_delta):
	"""Update telemetry displays"""
	if target and target is LCRoverJointController:
		_update_wheel_telemetry()

func _on_speed_changed(speed: float):
	if speed_label:
		speed_label.text = "%.1f m/s" % speed

func _on_motor_changed(power: float):
	if motor_value:
		motor_value.text = "%.0f%%" % (power * 100)

func _on_steering_changed(angle: float):
	if steering_value:
		steering_value.text = "%.2f" % angle

func _on_wheel_control_changed(wheel_name: String, motor: float, brake: float, steering: float):
	"""Update UI when wheel control changes"""
	# This is called when wheel controls are set programmatically
	# Update the corresponding sliders
	pass

func _update_mode_display():
	"""Update the mode label"""
	if mode_label and target:
		var mode_name = drive_modes[target.drive_mode]
		mode_label.text = mode_name

func _update_wheel_controls_visibility():
	"""Show/hide individual wheel controls based on mode"""
	if not scroll_container or not panel_container:
		return
	
	var show_individual = target and target.enable_individual_control and target.drive_mode == 3
	
	# Change visibility of the ScrollContainer itself
	scroll_container.visible = show_individual
	
	if show_individual:
		# MAXIMIZE: Explicitly set minimum height to fill screen
		var viewport_height = get_viewport_rect().size.y
		var target_height = viewport_height - 100.0 # 20px margin
		panel_container.custom_minimum_size.y = target_height
	else:
		# MINIMIZE: Reset minimum height to allow auto-shrink
		panel_container.custom_minimum_size.y = 0.0
	
	# Force layout update
	if vbox_container:
		vbox_container.queue_sort()
	
	# Reset size to ensure it recalculates correctly
	panel_container.size = Vector2.ZERO
	panel_container.reset_size()

func _update_wheel_telemetry():
	"""Update wheel telemetry displays"""
	if not target:
		return
	
	_update_wheel_rpm("front_left", fl_rpm_label)
	_update_wheel_rpm("front_right", fr_rpm_label)
	_update_wheel_rpm("back_left", bl_rpm_label)
	_update_wheel_rpm("back_right", br_rpm_label)

func _update_wheel_rpm(wheel_name: String, label: Label):
	"""Update RPM label for a specific wheel"""
	if not label:
		return
	
	var telemetry = target.get_wheel_telemetry(wheel_name)
	if telemetry.has("rpm"):
		label.text = "RPM: %.0f" % telemetry["rpm"]

# ============================================================================
# UI Callbacks
# ============================================================================

func _on_mode_selector_item_selected(index: int):
	"""Called when drive mode is changed"""
	if target:
		target.drive_mode = index
		# Enable individual control when Independent mode (index 3) is selected
		target.enable_individual_control = (index == 3)
		_update_mode_display()
		_update_wheel_controls_visibility()

func _on_settings_button_toggled(toggled_on: bool):
	print("DEBUG: Settings toggled: ", toggled_on)
	if parameter_editor:
		parameter_editor.visible = toggled_on
		if toggled_on:
			# Hide wheel controls when settings are open to save space
			if scroll_container:
				scroll_container.visible = false
			
			# MAXIMIZE: Explicitly set minimum height to fill screen (Same as wheel controls)
			var viewport_height = get_viewport_rect().size.y
			var target_height = viewport_height - 100.0 # 20px margin
			if panel_container:
				panel_container.custom_minimum_size.y = target_height
			
			# Initialize editor if needed
			if target:
				# Find the root vehicle to scan everything
				var root = target.parent if target.parent else target
				print("DEBUG: Setting parameter editor target: ", root)
				parameter_editor.set_target(root)
			else:
				print("DEBUG: No target for parameter editor")
		else:
			# Restore wheel controls visibility
			_update_wheel_controls_visibility()

# Front Left Wheel
func _on_fl_motor_slider_value_changed(value: float):
	if target:
		target.set_wheel_motor("front_left", value)

func _on_fl_brake_slider_value_changed(value: float):
	if target:
		target.set_wheel_brake("front_left", value)

func _on_fl_steering_slider_value_changed(value: float):
	if target:
		target.set_wheel_steering("front_left", value)

# Front Right Wheel
func _on_fr_motor_slider_value_changed(value: float):
	if target:
		target.set_wheel_motor("front_right", value)

func _on_fr_brake_slider_value_changed(value: float):
	if target:
		target.set_wheel_brake("front_right", value)

func _on_fr_steering_slider_value_changed(value: float):
	if target:
		target.set_wheel_steering("front_right", value)

# Back Left Wheel
func _on_bl_motor_slider_value_changed(value: float):
	if target:
		target.set_wheel_motor("back_left", value)

func _on_bl_brake_slider_value_changed(value: float):
	if target:
		target.set_wheel_brake("back_left", value)

func _on_bl_steering_slider_value_changed(value: float):
	if target:
		target.set_wheel_steering("back_left", value)

# Back Right Wheel
func _on_br_motor_slider_value_changed(value: float):
	if target:
		target.set_wheel_motor("back_right", value)

func _on_br_brake_slider_value_changed(value: float):
	if target:
		target.set_wheel_brake("back_right", value)

func _on_br_steering_slider_value_changed(value: float):
	if target:
		target.set_wheel_steering("back_right", value)
