extends Control

onready var player = $"../Spacecraft"

onready var position_lbl = $"HUD/Position"
onready var direction_lbl = $"HUD/Direction"

onready var velocity_lbl = $"HUD/Velocity"
onready var angvelocity_lbl = $"HUD/AngVelocity"

onready var acceleration_lbl = $"HUD/Acceleration"

# Declare member variables here. Examples:
# var a = 2
# var b = "text"

var prev_velocity = Vector3.ZERO

# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(delta):
#	pass


func _on_UpdateUI_timeout():
	position_lbl.text = "Position: " + str(player.transform.origin) + " Abs: " + str(player.transform.origin.length())
	direction_lbl.text = "Orientation: " + str(player.rotation)
	
	velocity_lbl.text = "Velocity: " + str(player.linear_velocity) + " Abs: " + str(player.linear_velocity.length())
	angvelocity_lbl.text = "AngVelocity: " + str(player.angular_velocity) + " Abs: " + str(player.angular_velocity.length())
	
	#change 100 to delta
	var acc = (player.linear_velocity - prev_velocity) / 100
	acceleration_lbl.text = "Acceleration: " + str(acc) + " Abs: " + str(acc.length())
	prev_velocity = player.linear_velocity
	
