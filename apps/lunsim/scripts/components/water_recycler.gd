@tool
extends "res://apps/lunsim/scripts/components/base_component.gd"

class_name WaterRecycler

var is_active: bool = false
var efficiency: float = 0.85  # 85% efficiency
var recovery_rate: float = 0.95  # 95% water recovery

func _init():
	super()
	name = "WaterRecycler"
	title = "Water Recycler"
	set_component_tooltip("Recycles and purifies water for the colony. Requires electricity to operate.")

func setup_component():
	# Water recyclers need electricity to operate
	max_electricity = 30.0
	max_oxygen = 0.0
	max_water = 150.0
	
	# Set consumption/production rates
	electricity_consumption_rate = 4.0  # 4 kW
	oxygen_production_rate = 0.0
	water_production_rate = 1.5  # 1.5 L/s when running
	
	# Add a container to show water recycler status
	var vbox = VBoxContainer.new()
	add_child(vbox)
	
	# Use a placeholder emoji instead of preloading the icon
	var placeholder = Label.new()
	placeholder.text = "💧"
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
	
	# Add water bar
	var water_label = Label.new()
	water_label.text = "Water Storage"
	vbox.add_child(water_label)
	
	var water_bar = ProgressBar.new()
	water_bar.name = "WaterBar"
	water_bar.max_value = max_water
	water_bar.value = 0
	water_bar.show_percentage = true
	water_bar.tooltip_text = "Current water level in the component"
	vbox.add_child(water_bar)
	
	# Add info label
	var info_label = Label.new()
	info_label.name = "InfoLabel"
	info_label.text = "Power: %.1f kW\nWater Output: %.1f L/s\nEfficiency: %.0f%%" % [
		electricity_consumption_rate,
		water_production_rate,
		efficiency * 100
	]
	vbox.add_child(info_label)

func _setup_slots():
	# Water recyclers take electricity and water input, and output water
	set_slot(0, true, ResourceType.ELECTRICITY, _get_slot_color(ResourceType.ELECTRICITY), 
			false, ResourceType.ELECTRICITY, Color(0, 0, 0))
	set_slot(1, true, ResourceType.WATER, _get_slot_color(ResourceType.WATER),
			false, ResourceType.WATER, Color(0, 0, 0))
	set_slot(2, false, ResourceType.WATER, Color(0, 0, 0),
			true, ResourceType.WATER, _get_slot_color(ResourceType.WATER))
			
	# Add tooltips to the slots
	var slots_container = get_child(0)
	if slots_container and slots_container is VBoxContainer:
		if slots_container.get_child_count() >= 3:
			slots_container.get_child(0).tooltip_text = "Connect to a power source"
			slots_container.get_child(1).tooltip_text = "Connect to a water source for recycling"
			slots_container.get_child(2).tooltip_text = "Connect to components that need water"

func simulate(delta: float):
	# Check if we have enough power to operate
	is_active = stored_electricity >= electricity_consumption_rate * delta
	
	if is_active:
		# Process water recycling
		# First, consume resources using parent method
		super.simulate(delta)
		
		# Then produce water based on recycling
		var actual_production = water_production_rate * efficiency * delta
		
		# Add processed water to storage
		stored_water += actual_production
		stored_water = min(stored_water, max_water)
	else:
		# Not enough power - don't produce water
		water_production_rate = 0.0
		_update_display()

func _update_display():
	# Update status label and color
	var status_label = get_node_or_null("StatusLabel")
	if status_label:
		if is_active:
			status_label.text = "Status: Recycling"
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
	
	var water_bar = get_node_or_null("WaterBar")
	if water_bar:
		water_bar.value = stored_water
	
	# Update info label
	var info_label = get_node_or_null("InfoLabel")
	if info_label:
		var current_output = water_production_rate * efficiency if is_active else 0.0
		info_label.text = "Power: %.1f kW\nWater Output: %.1f L/s\nEfficiency: %.0f%%" % [
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
		1, 2:
			return ResourceType.WATER
		_:
			return ResourceType.ELECTRICITY 
