extends PanelContainer

## Enhanced Component Inspector with integrated settings, resources, and effectors
## Provides a unified interface for inspecting and controlling vehicles/rovers

@onready var component_tree = $MainVBox/MainScroll/ContentVBox/ComponentTreeSection/TreeScroll/ComponentTree
@onready var settings_content = $MainVBox/MainScroll/ContentVBox/SettingsSection/SettingsContent
@onready var telemetry_grid = $MainVBox/MainScroll/ContentVBox/TelemetrySection/TelemetryGrid
@onready var effectors_grid = $MainVBox/MainScroll/ContentVBox/EffectorsSection/EffectorsGrid
@onready var resource_monitor = $MainVBox/MainScroll/ContentVBox/ResourceSection/ResourceMonitor

# Collapsible section headers
@onready var resource_header = $MainVBox/MainScroll/ContentVBox/ResourceSection/ResourceHeader
@onready var tree_header = $MainVBox/MainScroll/ContentVBox/ComponentTreeSection/TreeHeader
@onready var settings_header = $MainVBox/MainScroll/ContentVBox/SettingsSection/SettingsHeader
@onready var effectors_header = $MainVBox/MainScroll/ContentVBox/EffectorsSection/EffectorsHeader
@onready var telemetry_header = $MainVBox/MainScroll/ContentVBox/TelemetrySection/TelemetryHeader

var selected_rover: Node = null
var selected_component: Node = null
var update_timer = 0.0

func _ready():
	print("ComponentInspector: Enhanced version ready")
	
	# Connect to BuilderManager
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		print("ComponentInspector: Connected to BuilderManager")
		bm.entity_selected.connect(set_selected_rover)
	else:
		print("ComponentInspector: BuilderManager not found")
	
	# Connect tree selection
	component_tree.item_selected.connect(_on_component_selected)
	
	# Connect collapsible headers
	resource_header.toggled.connect(_on_section_toggled.bind("resource"))
	tree_header.toggled.connect(_on_section_toggled.bind("tree"))
	settings_header.toggled.connect(_on_section_toggled.bind("settings"))
	effectors_header.toggled.connect(_on_section_toggled.bind("effectors"))
	telemetry_header.toggled.connect(_on_section_toggled.bind("telemetry"))

func _on_section_toggled(pressed: bool, section: String):
	match section:
		"resource":
			resource_monitor.visible = pressed
		"tree":
			$MainVBox/MainScroll/ContentVBox/ComponentTreeSection/TreeScroll.visible = pressed
		"settings":
			settings_content.visible = pressed
		"effectors":
			effectors_grid.visible = pressed
		"telemetry":
			telemetry_grid.visible = pressed

func _process(delta):
	# Update structure view periodically
	update_timer += delta
	if update_timer > 0.5:  # Update twice per second
		update_timer = 0.0
		update_structure_view()
		_update_telemetry()

func update_structure_view():
	component_tree.clear()
	var root = component_tree.create_item()
	
	if selected_rover and is_instance_valid(selected_rover):
		var rover_item = component_tree.create_item(root)
		rover_item.set_metadata(0, selected_rover)
		
		# Add components as children
		var components = []
		if selected_rover is LCConstructible:
			components = selected_rover.components
		elif selected_rover is LCVehicle:
			components = selected_rover.state_effectors
			
		for comp in components:
			var comp_item = component_tree.create_item(rover_item)
			var mass_val = 0.0
			if comp.has_method("get_mass_contribution"):
				mass_val = comp.get_mass_contribution()
			elif "mass" in comp:
				mass_val = comp.mass
				
			comp_item.set_text(0, "  └ " + comp.name + " (%.1f kg)" % mass_val)
			comp_item.set_metadata(0, comp)
		
		# Show total mass
		var total_mass = selected_rover.mass
		rover_item.set_text(0, selected_rover.name + " (Total: %.1f kg, %d parts)" % [total_mass, components.size()])
		
		# Expand to show components
		rover_item.collapsed = false
	else:
		var item = component_tree.create_item(root)
		item.set_text(0, "No rover selected")

func set_selected_rover(rover: Node):
	print("ComponentInspector: set_selected_rover called with ", rover)
	selected_rover = rover
	
	# Update resource monitor
	if resource_monitor:
		if rover is LCVehicle:
			resource_monitor.set_vehicle(rover)
		else:
			resource_monitor.set_vehicle(null)
	
	update_structure_view()
	_update_effectors()
	
	if rover:
		show_component_info(rover)
	else:
		# Clear everything
		_clear_settings()
		_clear_telemetry()

func _on_component_selected():
	var selected = component_tree.get_selected()
	if selected:
		var obj = selected.get_metadata(0)
		selected_component = obj
		show_component_info(obj)

func show_component_info(obj):
	# Clear previous content
	_clear_settings()
	_clear_telemetry()
	
	# Show settings if component has Parameters
	if obj and "Parameters" in obj and obj.Parameters is Dictionary and not obj.Parameters.is_empty():
		_create_parameter_controls(obj)
	
	# Show telemetry
	_update_telemetry()

func _create_parameter_controls(component: Object):
	# Create interactive controls for component parameters
	var header = Label.new()
	header.text = "Settings for: " + component.name
	header.add_theme_font_size_override("font_size", 14)
	header.add_theme_color_override("font_color", Color(0.6, 0.8, 1.0))
	settings_content.add_child(header)
	
	settings_content.add_child(HSeparator.new())
	
	# Create controls for each parameter
	for param_key in component.Parameters:
		var metadata = component.Parameters[param_key]
		_create_parameter_control(component, param_key, metadata)

func _create_parameter_control(component: Object, param_key: String, metadata: Dictionary):
	var row = HBoxContainer.new()
	settings_content.add_child(row)
	
	var label = Label.new()
	label.text = param_key.capitalize()
	label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	label.custom_minimum_size.x = 120
	row.add_child(label)
	
	var property_path = metadata.get("path", "")
	if property_path.is_empty():
		return # Invalid metadata
		
	var current_value = component.get(property_path)
	var type = metadata.get("type", "float")
	
	if type == "float" or type == "int":
		var slider = HSlider.new()
		slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		slider.min_value = metadata.get("min", 0.0)
		slider.max_value = metadata.get("max", 100.0)
		slider.step = metadata.get("step", 0.1 if type == "float" else 1.0)
		slider.value = current_value
		
		var value_label = Label.new()
		value_label.text = str(current_value)
		value_label.custom_minimum_size.x = 60
		
		slider.value_changed.connect(func(val):
			component.set(property_path, val)
			value_label.text = ("%.2f" % val) if type == "float" else str(int(val))
			# Optional: Trigger update if component needs it
			if component.has_method("_update_parameters"):
				component._update_parameters()
		)
		
		row.add_child(slider)
		row.add_child(value_label)
		
	elif type == "bool":
		var checkbox = CheckBox.new()
		checkbox.button_pressed = current_value
		checkbox.toggled.connect(func(val):
			component.set(property_path, val)
		)
		row.add_child(checkbox)

func _update_effectors():
	# Clear existing effector panels
	for child in effectors_grid.get_children():
		child.queue_free()
	
	if not selected_rover or not selected_rover is LCVehicle:
		return
	
	# Find all effectors
	var effectors = []
	effectors.append_array(selected_rover.state_effectors)
	effectors.append_array(selected_rover.dynamic_effectors)
	
	# Remove duplicates
	var unique_effectors = []
	for eff in effectors:
		if not unique_effectors.has(eff):
			unique_effectors.append(eff)
	
	if unique_effectors.is_empty():
		var no_effectors = Label.new()
		no_effectors.text = "No effectors found"
		no_effectors.add_theme_color_override("font_color", Color(0.7, 0.7, 0.7))
		effectors_grid.add_child(no_effectors)
		return
	
	# Create compact effector panels
	for eff in unique_effectors:
		var panel = LCEffectorPanel.new()
		effectors_grid.add_child(panel)
		panel.setup(eff)

func _update_telemetry():
	if not selected_component:
		return
	
	# Clear existing telemetry
	for child in telemetry_grid.get_children():
		child.queue_free()
	
	var obj = selected_component
	
	# Show telemetry based on object type
	if obj is LCConstructible or obj is LCVehicle:
		if obj.has_method("get_telemetry_data"):
			var telemetry = obj.get_telemetry_data()
			if telemetry.size() > 0:
				for comp_name in telemetry:
					var comp_data = telemetry[comp_name]
					for key in comp_data:
						_add_telemetry_item(comp_name + "." + key, str(comp_data[key]))
						
	elif obj is LCComponent:
		# Show XTCE telemetry
		if obj.Telemetry.size() > 0:
			for key in obj.Telemetry:
				_add_telemetry_item(key, str(obj.Telemetry[key]))

func _add_telemetry_item(name: String, value: String):
	var name_label = Label.new()
	name_label.text = name + ":"
	name_label.add_theme_color_override("font_color", Color(0.8, 0.8, 0.8))
	telemetry_grid.add_child(name_label)
	
	var value_label = Label.new()
	value_label.text = value
	telemetry_grid.add_child(value_label)

func _clear_settings():
	for child in settings_content.get_children():
		child.queue_free()

func _clear_telemetry():
	for child in telemetry_grid.get_children():
		child.queue_free()

func count_wheels(constructible: Node) -> int:
	var count = 0
	for child in constructible.get_children():
		if child is VehicleWheel3D:
			count += 1
	return count
