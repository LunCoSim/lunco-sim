extends Control

var target: LCRoverController

@onready var speed_label = $Panel/VBoxContainer/SpeedLabel
@onready var steering_label = $Panel/VBoxContainer/SteeringLabel
@onready var motor_label = $Panel/VBoxContainer/MotorLabel

func _ready():
	pass

func set_target(_target):
	if _target is LCRoverController:
		target = _target
		target.speed_changed.connect(_on_speed_changed)
		target.steering_changed.connect(_on_steering_changed)
		target.motor_state_changed.connect(_on_motor_changed)
		print("RoverUI: Connected to rover controller")
	else:
		push_warning("RoverUI: Target is not a rover controller")

func _on_speed_changed(speed: float):
	speed_label.text = "Speed: %.1f m/s" % speed

func _on_steering_changed(angle: float):
	steering_label.text = "Steering: %.2f" % angle

func _on_motor_changed(power: float):
	motor_label.text = "Motor: %.0f%%" % (power * 100) 