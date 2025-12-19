class_name ResourceEditor
extends Control

@onready var resource_list: VBoxContainer = %ResourceList
@onready var add_button: Button = $VBoxContainer/AddResourceButton

func _ready() -> void:
	add_button.pressed.connect(_on_add_resource_pressed)
	refresh_list()

func refresh_list() -> void:
	# Clear existing resources
	for child in resource_list.get_children():
		child.queue_free()
	
	# Add new resource buttons
	# Use LCResourceRegistry autoload
	for resource in LCResourceRegistry.get_all_resources():
		var button = Button.new()
		button.text = resource.display_name
		button.pressed.connect(func(): _on_resource_selected(resource))
		resource_list.add_child(button)

func _on_add_resource_pressed() -> void:
	var resource = LCResourceDefinition.new()
	resource.display_name = "New Resource"
	resource.resource_id = "new_resource_" + str(randi())
	LCResourceRegistry.register_resource(resource)
	refresh_list()

func _on_resource_selected(resource: LCResourceDefinition) -> void:
	# TODO: Implement resource selection and property editing
	pass
