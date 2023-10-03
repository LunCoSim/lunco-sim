extends Control

@onready var ui := $TargetUI

var _ui
# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass


# Function set_ui clears the ui and sets target if ui exists
func set_ui(_ui=null):
	clear_ui()
	if(_ui):
		ui.add_child(_ui)
		

# Function clear_ui removes child items if ui exists	
func clear_ui():
	if ui:
		for n in ui.get_children():
			ui.remove_child(n)

func set_target(target):
	
	if target is lnPlayer:
		_ui = preload("res://ui/player-ui.tscn").instantiate()
	elif target is lnSpacecraft:
		_ui = preload("res://ui/spacecraft-ui.tscn").instantiate()
	elif target is lnOperator:
		_ui = preload("res://ui/operator-ui.tscn").instantiate()
	
	#if _ui:
		#_ui.set_target(target) #controller specific function
	set_ui(_ui)
