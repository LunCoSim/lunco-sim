class_name ResourceEditor
extends Control

@onready var resource_list: ItemList = $VBoxContainer/ResourceList
@onready var add_button: Button = $VBoxContainer/ToolBar/AddResourceButton

var registry: ResourceRegistry

func _ready() -> void:
	registry = ResourceRegistry.get_instance()
	#add_button.pressed.connect(_on_add_resource_pressed)
	refresh_list()

func refresh_list() -> void:
	#resource_list.clear()
	for resource in registry.get_all_resources():
		resource_list.add_item(resource.name)

func _on_add_resource_pressed() -> void:
	var resource = BaseResource.new()
	resource.name = "New Resource"
	registry.register_resource(resource)
	refresh_list()
