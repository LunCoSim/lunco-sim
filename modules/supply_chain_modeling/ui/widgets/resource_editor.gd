class_name ResourceEditor
extends Control

@onready var resource_list: ItemList = $HSplitContainer/VBoxContainer/ResourceList
@onready var add_button: Button = $HSplitContainer/VBoxContainer/ToolBar/AddResourceButton
@onready var property_editor: ResourcePropertyEditor = $HSplitContainer/PropertyEditor

var registry: ResourceRegistry

func _ready() -> void:
	registry = ResourceRegistry.get_instance()
	add_button.pressed.connect(_on_add_resource_pressed)
	resource_list.item_selected.connect(_on_resource_selected)
	property_editor.property_changed.connect(_on_property_changed)
	refresh_list()

func refresh_list() -> void:
	resource_list.clear()
	for resource in registry.get_all_resources():
		resource_list.add_item(resource.name)

func _on_add_resource_pressed() -> void:
	var resource = BaseResource.new()
	resource.name = "New Resource"
	registry.register_resource(resource)
	refresh_list()
	# Select the new resource
	resource_list.select(resource_list.item_count - 1)
	_on_resource_selected(resource_list.item_count - 1)

func _on_resource_selected(index: int) -> void:
	var resource_name = resource_list.get_item_text(index)
	var resource = registry.get_resource(resource_name)
	if resource:
		property_editor.edit_resource(resource)

func _on_property_changed(property: String, value: Variant) -> void:
	var selected_idx = resource_list.get_selected_items()[0]
	var resource_name = resource_list.get_item_text(selected_idx)
	var resource = registry.get_resource(resource_name)
	if resource:
		resource.set(property, value)
		if property == "name":
			refresh_list()
			# Reselect the item
			for i in resource_list.item_count:
				if resource_list.get_item_text(i) == value:
					resource_list.select(i)
					break
