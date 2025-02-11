class_name ResourceEditor
extends Control

@onready var resource_list: VBoxContainer = %ResourceList
@onready var add_button: Button = $VBoxContainer/AddResourceButton

var registry: ResourceRegistry

func _ready() -> void:
	registry = ResourceRegistry.get_instance()
	add_button.pressed.connect(_on_add_resource_pressed)
	refresh_list()

func refresh_list() -> void:
	# Clear existing resources
	for child in resource_list.get_children():
		child.queue_free()
	
	# Add new resource buttons
	for resource in registry.get_all_resources():
		var button = Button.new()
		button.text = resource.name
		button.pressed.connect(func(): _on_resource_selected(resource))
		resource_list.add_child(button)

func _on_add_resource_pressed() -> void:
	var resource = BaseResource.new()
	resource.name = "New Resource"
	registry.register_resource(resource)
	refresh_list()

func _on_resource_selected(resource: BaseResource) -> void:
	# TODO: Implement resource selection and property editing
	pass
