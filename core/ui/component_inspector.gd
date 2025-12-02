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
	# Update structure view and telemetry periodically
	update_timer += delta
	if update_timer > 2.0:  # Update every 2 seconds
		update_timer = 0.0
		update_structure_view()
		_update_telemetry_values()  # Only update values, don't rescan

func update_structure_view():
	var root = component_tree.get_root()
	if not root:
		root = component_tree.create_item()
	
	if not selected_rover or not is_instance_valid(selected_rover):
		# Check if we are already showing "No rover selected"
		if root.get_child_count() > 0:
			var first_child = root.get_child(0)
			if first_child.get_text(0) == "No rover selected":
				return
		
		component_tree.clear()
		root = component_tree.create_item()
		var item = component_tree.create_item(root)
		item.set_text(0, "No rover selected")
		return

	# Get components list first
	var components = []
	if not selected_rover:
		return

	if selected_rover is LCConstructible:
		components = selected_rover.components
	elif selected_rover is LCVehicle or selected_rover is LCSpacecraft or selected_rover.has_method("_on_spacecraft_controller_thrusted") or selected_rover.has_method("set_control_inputs") or (selected_rover.get_script() and selected_rover.get_script().resource_path.ends_with("spacecraft.gd")):
		components = []
		components.append_array(selected_rover.state_effectors)
		if "dynamic_effectors" in selected_rover:
			for eff in selected_rover.dynamic_effectors:
				if not components.has(eff):
					components.append(eff)

	# Check if we need to rebuild
	var rebuild = false
	var rover_item = null
	
	if root.get_child_count() == 0:
		rebuild = true
	else:
		rover_item = root.get_child(0)
		# Check if it's the same rover object
		if rover_item.get_metadata(0) != selected_rover:
			rebuild = true
		# Check if component count changed
		elif rover_item.get_child_count() != components.size():
			rebuild = true
	
	if rebuild:
		component_tree.clear()
		root = component_tree.create_item()
		rover_item = component_tree.create_item(root)
		rover_item.set_metadata(0, selected_rover)
		rover_item.collapsed = false
		
		# Create items
		for comp in components:
			var comp_item = component_tree.create_item(rover_item)
			comp_item.set_metadata(0, comp)
			_update_component_item_text(comp_item, comp)
			
		# Update rover text
		_update_component_item_text(rover_item, selected_rover, components.size())
	else:
		# Just update text of existing items
		_update_component_item_text(rover_item, selected_rover, components.size())
		
		var i = 0
		var child = rover_item.get_child(0)
		while child:
			if i < components.size():
				_update_component_item_text(child, components[i])
			child = child.get_next()
			i += 1

func _update_component_item_text(item: TreeItem, obj: Object, count: int = -1):
	if obj == selected_rover:
		var total_mass = 0.0
		if "mass" in obj:
			total_mass = obj.mass
		item.set_text(0, obj.name + " (Total: %.1f kg, %d parts)" % [total_mass, count])
	else:
		var mass_val = 0.0
		if obj.has_method("get_mass_contribution"):
			mass_val = obj.get_mass_contribution()
		elif "mass" in obj:
			mass_val = obj.mass
			
		item.set_text(0, "  └ " + obj.name + " (%.1f kg)" % mass_val)

func set_selected_rover(rover: Node):
	print("ComponentInspector: set_selected_rover called with ", rover)
	selected_rover = rover
	
	# Update resource monitor
	if resource_monitor:
		var is_spacecraft = false
		if rover:
			is_spacecraft = rover is LCVehicle or rover is LCSpacecraft or rover.has_method("_on_spacecraft_controller_thrusted") or rover.has_method("set_control_inputs") or (rover.get_script() and rover.get_script().resource_path.ends_with("spacecraft.gd"))
		
		if is_spacecraft:
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
	
	if obj:
		# If it's the root vehicle/rover, scan entire hierarchy
		var is_spacecraft = obj is LCConstructible or obj is LCVehicle or obj is LCSpacecraft or obj.has_method("_on_spacecraft_controller_thrusted") or obj.has_method("set_control_inputs") or (obj.get_script() and obj.get_script().resource_path.ends_with("spacecraft.gd"))
		if is_spacecraft:
			print("ComponentInspector: Scanning entire vehicle hierarchy for Parameters...")
			_scan_and_create_settings(obj)
		# If it's a specific component, show only its settings
		elif "Parameters" in obj and obj.Parameters is Dictionary and not obj.Parameters.is_empty():
			print("ComponentInspector: Showing settings for selected component: ", obj.name)
			_create_parameter_controls(obj)
		else:
			print("ComponentInspector: Selected component has no Parameters")
	
	# Initial telemetry display (create labels)
	_update_telemetry_display()

func _scan_and_create_settings(node: Node):
	"""Recursively scan node and its children for Parameters"""
	# Check if this node has Parameters
	if "Parameters" in node and node.Parameters is Dictionary and not node.Parameters.is_empty():
		print("ComponentInspector: Found component with Parameters: ", node.name)
		_create_parameter_controls(node)
	
	# Recursively scan children
	for child in node.get_children():
		_scan_and_create_settings(child)

func _create_parameter_controls(component: Object):
	# Create header
	var header = Label.new()
	header.text = "Settings for: " + component.name
	header.add_theme_font_size_override("font_size", 14)
	header.add_theme_color_override("font_color", Color(0.6, 0.8, 1.0))
	settings_content.add_child(header)
	
	settings_content.add_child(HSeparator.new())
	
	# Use LCParameterEditor to create controls
	var param_editor = LCParameterEditor.new()
	param_editor.target_node = component
	settings_content.add_child(param_editor)
	param_editor.refresh()

func _update_effectors():
	# Clear existing effector panels
	for child in effectors_grid.get_children():
		child.queue_free()
	
	if not selected_rover:
		return

	var is_spacecraft = selected_rover is LCVehicle or selected_rover is LCSpacecraft or selected_rover.has_method("_on_spacecraft_controller_thrusted") or selected_rover.has_method("set_control_inputs") or (selected_rover.get_script() and selected_rover.get_script().resource_path.ends_with("spacecraft.gd"))
	if not is_spacecraft:
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

func _update_telemetry_display():
	"""Initial setup of telemetry display - creates labels"""
	if not selected_component:
		return
	
	# Clear existing telemetry
	for child in telemetry_grid.get_children():
		child.queue_free()
	
	var obj = selected_component
	if not obj:
		return

	# Show telemetry based on object type
	var is_spacecraft = obj is LCConstructible or obj is LCVehicle or obj is LCSpacecraft or obj.has_method("_on_spacecraft_controller_thrusted") or obj.has_method("set_control_inputs") or (obj.get_script() and obj.get_script().resource_path.ends_with("spacecraft.gd"))
	if is_spacecraft:
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

func _update_telemetry_values():
	"""Fast update of telemetry values only - doesn't recreate UI"""
	if not selected_component or not telemetry_grid.visible:
		return
	
	# Only update if telemetry section is expanded
	var obj = selected_component
	var child_index = 0
	
	# Update existing labels with new values
	if obj is LCComponent and obj.Telemetry.size() > 0:
		for key in obj.Telemetry:
			# Skip the name label, update the value label
			if child_index + 1 < telemetry_grid.get_child_count():
				var value_label = telemetry_grid.get_child(child_index + 1)
				if value_label is Label:
					value_label.text = str(obj.Telemetry[key])
			child_index += 2  # Skip name and value labels

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
