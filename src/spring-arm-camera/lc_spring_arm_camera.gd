class_name LcSpringArmCamera
extends Node3D

@export var target_path : NodePath

#---------

enum Commands {
	SET_TARGET,
	RESET_TARGET,
	
	START_ROTATION,
	STOP_ROTATION,
	
	SET_SPRING_LENGTH
}

#---------

@onready var RotX = $RotX
@onready var RotY = $RotX/RotY
@onready var Camera = $RotX/RotY/Camera

#---------
@onready var target = get_node(target_path)

var rotation_direction := Vector2.ZERO

#---------
#---------
# Called when the node enters the scene tree for the first time.
func _ready():
	pass

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	if target:
		var targetPosition = target.position
		position = target.position
	
	if rotation_direction.length():
		RotX.rotation.x += rotation_direction.y * delta
		rotation.y += rotation_direction.x * delta

#---------

func execute_command(command: Commands):
	return execute_command_args(command, null)

func execute_command_args(command: Commands, args):
	match command:
		Commands.SET_TARGET:
			target = args
		Commands.RESET_TARGET:
			target = null
		Commands.START_ROTATION:
			rotation_direction = args
		Commands.STOP_ROTATION:
			rotation_direction = Vector2.ZERO
		Commands.SET_SPRING_LENGTH:
			RotY.spring_length = args
		_:
			print("Unknown commad")
