extends PanelContainer

@onready var component_tree = $VBoxContainer/ScrollContainer/ComponentTree
@onready var properties_grid = $VBoxContainer/PropertiesScroll/PropertiesGrid

var selected_rover: LCConstructible = null
var update_timer = 0.0

func _ready():
	# Setup component tree
	component_tree.item_selected.connect(_on_component_selected)

func _process(delta):
	# Update structure view periodically
	update_timer += delta
	if update_timer > 0.5:  # Update twice per second
		update_timer = 0.0
		update_structure_view()

func update_structure_view():
	component_tree.clear()
	var root = component_tree.create_item()
	
	if selected_rover and is_instance_valid(selected_rover):
		var rover_item = component_tree.create_item(root)
		rover_item.set_text(0, selected_rover.name)
		rover_item.set_metadata(0, selected_rover)
		
		# Add components as children
		for comp in selected_rover.components:
			var comp_item = component_tree.create_item(rover_item)
			comp_item.set_text(0, "  └ " + comp.name + " (%.1f kg)" % comp.mass)
			comp_item.set_metadata(0, comp)
		
		# Show total mass
		var total_mass = selected_rover.mass
		rover_item.set_text(0, selected_rover.name + " (Total: %.1f kg, %d parts)" % [total_mass, selected_rover.components.size()])
		
		# Expand to show components
		rover_item.collapsed = false
	else:
		var item = component_tree.create_item(root)
		item.set_text(0, "No rover selected")

func set_selected_rover(rover: LCConstructible):
	selected_rover = rover
	update_structure_view()
	if rover:
		show_properties(rover)
	else:
		# Clear properties
		for child in properties_grid.get_children():
			child.queue_free()

func _on_component_selected():
	var selected = component_tree.get_selected()
	if selected:
		var obj = selected.get_metadata(0)
		show_properties(obj)

func show_properties(obj):
	# Clear existing properties
	for child in properties_grid.get_children():
		child.queue_free()
	
	if obj is LCConstructible:
		selected_rover = obj
		add_property_label("=== Rover ===")
		add_property("Name", obj.name)
		add_property("Mass", "%.1f kg" % obj.mass)
		add_property("Components", str(obj.components.size()))
		add_property("Wheels", str(count_wheels(obj)))
		
		# Add XTCE telemetry if available
		var telemetry = obj.get_telemetry_data()
		if telemetry.size() > 0:
			add_property_label("=== Telemetry ===")
			for comp_name in telemetry:
				var comp_data = telemetry[comp_name]
				for key in comp_data:
					add_property(comp_name + "." + key, str(comp_data[key]))
					
	elif obj is LCComponent:
		add_property_label("=== Component ===")
		add_property("Name", obj.name)
		add_property("Mass", "%.1f kg" % obj.mass)
		if obj.power_consumption > 0:
			add_property("Power Use", "%.1f W" % obj.power_consumption)
		if obj.power_production > 0:
			add_property("Power Gen", "%.1f W" % obj.power_production)
		
		# Show XTCE telemetry
		if obj.Telemetry.size() > 0:
			add_property_label("=== Telemetry ===")
			for key in obj.Telemetry:
				add_property(key, str(obj.Telemetry[key]))
		
		# Show XTCE commands
		if obj.Commands.size() > 0:
			add_property_label("=== Commands ===")
			for key in obj.Commands:
				add_property(key, str(obj.Commands[key]))

func add_property_label(text: String):
	var label = Label.new()
	label.text = text
	label.add_theme_color_override("font_color", Color(0.8, 0.8, 1.0))
	properties_grid.add_child(label)
	properties_grid.add_child(Label.new())  # Empty cell

func add_property(name: String, value: String):
	var name_label = Label.new()
	name_label.text = name + ":"
	properties_grid.add_child(name_label)
	
	var value_label = Label.new()
	value_label.text = value
	properties_grid.add_child(value_label)

func count_wheels(constructible: LCConstructible) -> int:
	var count = 0
	for child in constructible.get_children():
		if child is VehicleWheel3D:
			count += 1
	return count
