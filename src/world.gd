extends Node

var mouse_control = false
const MOUSE_SENSITIVITY = 1.0
var camera_dir := Vector2.ZERO

# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.

func _input(event):
#	if Input.is_action_just_pressed("select_player"):
##		state.set_trigger("player")
#		pass
#	elif Input.is_action_just_pressed("select_spacecraft"):
#		state.set_trigger("spacecraft")
#	elif Input.is_action_just_pressed("select_operator"):
#		state.set_trigger("operator")
		
	if Input.is_action_pressed("rotate_camera"):
		Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
		mouse_control = true
	else:
		Input.set_mouse_mode(Input.MOUSE_MODE_VISIBLE)
		mouse_control = false
	
	if (event is InputEventMouseMotion) and mouse_control:
		camera_dir = event.relative * MOUSE_SENSITIVITY
	else:
		camera_dir = Input.get_vector("ui_left", "ui_right", "ui_up", "ui_down")

func _process(delta):
	var character: LcCharacter = $Character
	var camera: LcSpringArmCamera = $Camera
	
	if Input.is_action_just_pressed("ui_accept"):
		character.execute_command(LcCharacter.Commands.JUMP)
	
	# Get the input direction and handle the movement/deceleration.
	# As good practice, you should replace UI actions with custom gameplay actions.
	
	if camera_dir.length() > 0:
		camera.execute_command_args(LcSpringArmCamera.Commands.START_ROTATION, camera_dir)
	else:
		camera.execute_command(LcSpringArmCamera.Commands.STOP_ROTATION)
	
	
	# Get the input direction and handle the movement/deceleration.
	# As good practice, you should replace UI actions with custom gameplay actions.
	var move_dir = Input.get_vector("move_left", "move_right", "move_forward", "move_backwards")
	
	if move_dir.length() > 0:
		character.execute_command_args(LcCharacter.Commands.START_MOVING, move_dir)
	else:
		character.execute_command(LcCharacter.Commands.STOP_MOVING)
