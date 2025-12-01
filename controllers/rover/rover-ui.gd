extends LCControllerUI

# target is inherited from LCControllerUI (typed as LCRoverController)

@onready var speed_label = $Help/SpeedLabel
@onready var steering_label = $Help/SteeringLabel
@onready var motor_label = $Help/MotorLabel

# UI update throttling
var update_timer := 0.0
const UPDATE_INTERVAL := 0.1  # 10 fps instead of 60

func _ready():
	pass

# Override base class hook to connect signals when target is set
func _on_target_set():
	if target is LCRoverController:
		target.speed_changed.connect(_on_speed_changed)
		target.steering_changed.connect(_on_steering_changed)
		target.motor_state_changed.connect(_on_motor_changed)
	else:
		push_warning("RoverUI: Target is not a rover controller")

func _process(delta):
	# Throttle UI updates to reduce performance impact
	update_timer += delta
	if update_timer >= UPDATE_INTERVAL:
		update_timer = 0.0
		_update_ui_labels()

func _on_speed_changed(speed: float):
	# Signal received - mark for update
	pass

func _on_steering_changed(angle: float):
	# Signal received - mark for update
	pass

func _on_motor_changed(power: float):
	# Signal received - mark for update
	pass

func _update_ui_labels():
	"""Update UI labels with current values"""
	if not target:
		return

	# Get current values from target
	var speed = target.current_speed if "current_speed" in target else 0.0
	var steering = target.steering_input if "steering_input" in target else 0.0
	var motor = target.motor_input if "motor_input" in target else 0.0

	# Update labels
	speed_label.text = "Speed: %.1f m/s" % speed
	steering_label.text = "Steering: %.2f" % steering
	motor_label.text = "Motor: %.0f%%" % (motor * 100)
