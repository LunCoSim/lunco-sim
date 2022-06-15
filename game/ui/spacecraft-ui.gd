extends Control

export (NodePath) var Target

onready var target = get_node(Target)

onready var position_lbl = $"HUD/Position"
onready var direction_lbl = $"HUD/Direction"

onready var velocity_lbl = $"HUD/Velocity"
onready var angvelocity_lbl = $"HUD/AngVelocity"

onready var acceleration_lbl = $"HUD/Acceleration"

var prev_velocity = Vector3.ZERO

func set_target(_target):
	target = _target
	
# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(delta):
#	pass


func _on_UpdateUI_timeout():
	if target:
		position_lbl.text = "Position: " + str(target.transform.origin) + " Abs: " + str(target.transform.origin.length())
		direction_lbl.text = "Orientation: " + str(target.rotation)
		
		velocity_lbl.text = "Velocity: " + str(target.linear_velocity) + " Abs: " + str(target.linear_velocity.length())
		angvelocity_lbl.text = "AngVelocity: " + str(target.angular_velocity) + " Abs: " + str(target.angular_velocity.length())
		
		#change 100 to delta
		var acc = (target.linear_velocity - prev_velocity) / 100
		acceleration_lbl.text = "Acceleration: " + str(acc) + " Abs: " + str(acc.length())
		prev_velocity = target.linear_velocity

func _on_HideControls_timeout():
	$Help.visible = false
	$MET.visible = true
	 
