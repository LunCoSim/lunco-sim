extends Control

@onready var ui := $TargetUI

var _ui
# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.
	
	var tree: Tree = $Entities
	var root = tree.create_item()  # This creates the root item.
	
	for entity in EntitiesDB.Entities:
		# Add child items to the root.
		var item = tree.create_item(root)
		var txt = "Entity: " + str(entity)
		item.set_text(0, txt)
	

	## Add a child to item1.
	#var subitem1 = self.create_item(item1)
	#subitem1.set_text(0, "Subitem 1.1")

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
