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
		var vec = target.transform.origin
		position_lbl.text = "Position: (%.2f, %.2f, %.2f) Abs: %.2f" % [vec.x, vec.y, vec.z, vec.length()]
		vec = target.rotation
		direction_lbl.text = "Orientation: (%.2f, %.2f, %.2f)" % [vec.x, vec.y, vec.z]
		
		vec = target.linear_velocity
		velocity_lbl.text = "Velocity: (%.2f, %.2f, %.2f) Abs: %.2f" % [vec.x, vec.y, vec.z, vec.length()]
		
		vec = target.angular_velocity
		angvelocity_lbl.text = "AngVelocity: (%.2f, %.2f, %.2f) Abs: %.2f" % [vec.x, vec.y, vec.z, vec.length()]
		
		#change 100 to delta
		var acc = (target.linear_velocity - prev_velocity) / 100
		acceleration_lbl.text = "Acceleration: (%.2f, %.2f, %.2f) Abs: %.2f" % [acc.x, acc.y, acc.z, acc.length()]
		prev_velocity = target.linear_velocity

func _on_HideControls_timeout():
	$Help.visible = false
	$MET.visible = true
	 
