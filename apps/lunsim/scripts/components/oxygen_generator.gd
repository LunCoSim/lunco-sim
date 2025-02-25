@tool
extends "res://apps/lunsim/scripts/components/base_component.gd"

class_name OxygenGenerator

var is_active: bool = false
var efficiency: float = 0.9  # 90% efficiency

func _init():
	super()
	name = "OxygenGenerator"
	title = "Oxygen Generator"
	set_component_tooltip("Produces oxygen for the colony. Requires electricity to operate.")

func setup_component():
	# Oxygen generators use electricity to create oxygen
	max_electricity = 30.0
	max_oxygen = 100.0
	max_water = 0.0
	
	# Set consumption/production rates
	electricity_consumption_rate = 5.0  # 5 kW
	oxygen_production_rate = 2.0  # 2 m³/s when running
	water_consumption_rate = 0.0
	
	# Add a container to show oxygen generator status
	var vbox = VBoxContainer.new()
	add_child(vbox)
	
	# Use a placeholder emoji instead of preloading the icon
	var placeholder = Label.new()
	placeholder.text = "💨"
	placeholder.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	placeholder.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	placeholder.add_theme_font_size_override("font_size", 32)
	vbox.add_child(placeholder)
	
	var label = Label.new()
	label.name = "StatusLabel"
	label.text = "Status: Inactive"
	vbox.add_child(label)
	
	# Add power bar
	var power_label = Label.new()
	power_label.text = "Power Storage"
	vbox.add_child(power_label)
	
	var power_bar = ProgressBar.new()
	power_bar.name = "PowerBar"
	power_bar.max_value = max_electricity
	power_bar.value = 0
	power_bar.show_percentage = true
	power_bar.tooltip_text = "Current power level in the component"
	vbox.add_child(power_bar)
	
	# Add oxygen bar
	var oxygen_label = Label.new()
	oxygen_label.text = "Oxygen Storage"
	vbox.add_child(oxygen_label)
	
	var oxygen_bar = ProgressBar.new()
	oxygen_bar.name = "OxygenBar"
	oxygen_bar.max_value = max_oxygen
	oxygen_bar.value = 0
	oxygen_bar.show_percentage = true
	oxygen_bar.tooltip_text = "Current oxygen level in the component"
	vbox.add_child(oxygen_bar)
	
	# Add info label
	var info_label = Label.new()
	info_label.name = "InfoLabel"
	info_label.text = "Power: %.1f kW\nO₂ Output: %.1f m³/s\nEfficiency: %.0f%%" % [
		electricity_consumption_rate,
		oxygen_production_rate,
		efficiency * 100
	]
	vbox.add_child(info_label)

func _setup_slots():
	# Oxygen generators take electricity and output oxygen
	set_slot(0, true, ResourceType.ELECTRICITY, _get_slot_color(ResourceType.ELECTRICITY), 
			false, ResourceType.ELECTRICITY, Color(0, 0, 0))
	set_slot(1, false, ResourceType.OXYGEN, Color(0, 0, 0),
			true, ResourceType.OXYGEN, _get_slot_color(ResourceType.OXYGEN))
			
	# Add tooltips to the slots
	var slots_container = get_child(0)
	if slots_container and slots_container is VBoxContainer:
		if slots_container.get_child_count() >= 2:
			slots_container.get_child(0).tooltip_text = "Connect to a power source"
			slots_container.get_child(1).tooltip_text = "Connect to components that need oxygen"

func simulate(delta: float):
	# Check if we have enough power to operate
	is_active = stored_electricity >= electricity_consumption_rate * delta
	
	if is_active:
		# Call parent simulation to handle resource consumption/production
		super.simulate(delta)
		
		# Apply efficiency to oxygen production
		var actual_production = oxygen_production_rate * efficiency * delta
		stored_oxygen += actual_production
		stored_oxygen = min(stored_oxygen, max_oxygen)
	else:
		# Not enough power - don't produce oxygen
		oxygen_production_rate = 0.0
		_update_display()

func _update_display():
	# Update status label and color
	var status_label = get_node_or_null("StatusLabel")
	if status_label:
		if is_active:
			status_label.text = "Status: Generating"
			status_label.add_theme_color_override("font_color", Color(0, 0.8, 0.2)) # Green for active
		else:
			if stored_electricity < electricity_consumption_rate * 0.1:  # 0.1 second worth
				status_label.text = "Status: No Power"
				status_label.add_theme_color_override("font_color", Color(0.9, 0.1, 0.1)) # Red for no power
			else:
				status_label.text = "Status: Inactive"
				status_label.add_theme_color_override("font_color", Color(0.9, 0.6, 0.1)) # Orange for inactive
	
	# Update progress bars
	var power_bar = get_node_or_null("PowerBar")
	if power_bar:
		power_bar.value = stored_electricity
		if stored_electricity < electricity_consumption_rate * 0.1:
			power_bar.modulate = Color(0.9, 0.1, 0.1) # Red for low power
		else:
			power_bar.modulate = Color(1, 1, 1) # Normal color
	
	var oxygen_bar = get_node_or_null("OxygenBar")
	if oxygen_bar:
		oxygen_bar.value = stored_oxygen
		oxygen_bar.modulate = Color(0.5, 0.8, 1.0) # Blue color for oxygen
	
	# Update info label
	var info_label = get_node_or_null("InfoLabel")
	if info_label:
		var current_output = oxygen_production_rate * efficiency if is_active else 0.0
		info_label.text = "Power: %.1f kW\nO₂ Output: %.1f m³/s\nEfficiency: %.0f%%" % [
			electricity_consumption_rate,
			current_output,
			efficiency * 100
		]
	
	# Update component status color
	update_component_status_color(is_active)

func _get_resource_type_from_port(port_index: int) -> int:
	match port_index:
		0:
			return ResourceType.ELECTRICITY
		1:
			return ResourceType.OXYGEN
		_:
			return ResourceType.ELECTRICITY 