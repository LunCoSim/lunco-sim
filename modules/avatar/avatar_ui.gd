extends Control

signal entity_selected(int)

@onready var ui := $TargetUI

var _ui
var avatar: LCAvatar
# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.
	
	var tree: ItemList = $Entities
	
	for entity in EntitiesDB.Entities:
		# Add child items to the root.
		tree.add_item("Entity: " + str(entity))
	
	avatar = get_parent()
	
	tree.select(avatar.entity_to_spawn)
	
	#var win: PankuLynxWindow = Panku.windows_manager.create_window($Entities)
#
	#var size = $Entities.get_minimum_size() + Vector2(0, win._window_title_container.get_minimum_size().y)
	#win.set_custom_minimum_size(size)
	#win.size = win.get_minimum_size()
	

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
	
	if target is LCCharacter:
		_ui = preload("res://controllers/character/character-ui.tscn").instantiate()
	elif target is LCSpacecraft:
		_ui = preload("res://controllers/spacecraft/spacecraft-ui.tscn").instantiate()
	elif target is LCOperator:
		_ui = preload("res://controllers/operator/operator-ui.tscn").instantiate()

	if _ui:
		_ui.set_target(target) #controller specific function
	set_ui(_ui)

func _on_entities_item_selected(index):
	print("_on_entities_item_selected: ", index)
	emit_signal("entity_selected", index)
	pass # Replace with function body.

func update_entities(entities):
	var tree: ItemList = $LiveEntities
	
	tree.clear()
	
	for entity in entities:
		# Add child items to the root.
		tree.add_item("Entity: " + str(entity))
	
