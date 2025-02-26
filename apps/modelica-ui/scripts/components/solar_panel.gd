@tool
extends "res://apps/lunsim/scripts/components/base_component.gd"

class_name SolarPanel

var efficiency: float = 0.25  # 25% efficiency
var panel_area: float = 10.0  # 10 square meters
var solar_irradiance: float = 1.0  # kW per square meter at 100% exposure

var day_night_cycle: float = 0.0
var day_length: float = 60.0  # 60 seconds per day for demonstration
var current_irradiance: float = 0.0

func _init():
	super()
	name = "SolarPanel"
	title = "Solar Panel"

func setup_component():
	# Solar panels only produce electricity
	max_electricity = 50.0
	max_oxygen = 0.0
	max_water = 0.0
	
	# No consumption rates
	electricity_consumption_rate = 0.0
	oxygen_consumption_rate = 0.0
	water_consumption_rate = 0.0
	
	# Add a container to show electricity levels
	var vbox = VBoxContainer.new()
	add_child(vbox)
	
	# Use a placeholder instead of trying to preload the icon
	var placeholder = Label.new()
	placeholder.text = "☀️"
	placeholder.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	placeholder.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	vbox.add_child(placeholder)
	
	var label = Label.new()
	label.name = "StatusLabel"
	label.text = "Status: Idle"
	vbox.add_child(label)
	
	# Add day/night indicator
	var daynight_label = Label.new()
	daynight_label.name = "DayNightLabel"
	daynight_label.text = "Day/Night: Day"
	vbox.add_child(daynight_label)
	
	var progress = ProgressBar.new()
	progress.name = "ElectricityBar"
	progress.max_value = max_electricity
	progress.value = 0
	progress.show_percentage = true
	vbox.add_child(progress)
	
	# Add spacer
	var spacer = Control.new()
	spacer.custom_minimum_size = Vector2(0, 10)
	vbox.add_child(spacer)
	
	var info_label = Label.new()
	info_label.name = "InfoLabel"
	info_label.text = "Output: 0.0 kW"
	vbox.add_child(info_label)
	
	# Add tooltip explaining the component
	set_component_tooltip("Solar Panel: Generates electricity during daylight hours.\nConnect to a Battery to store energy for night time.")

func _setup_slots():
	# Solar panels have no inputs, only outputs
	set_slot(0, false, ResourceType.ELECTRICITY, Color(0, 0, 0), true, ResourceType.ELECTRICITY, _get_slot_color(ResourceType.ELECTRICITY))

func simulate(delta: float):
	# Update day/night cycle
	day_night_cycle += delta
	if day_night_cycle > day_length:
		day_night_cycle = 0.0
	
	# Calculate current solar irradiance based on simple day/night cycle
	var time_of_day = day_night_cycle / day_length
	
	# Simple day/night curve: full power at noon, zero at night
	if time_of_day < 0.25 or time_of_day > 0.75:
		current_irradiance = 0.0  # Night time
	else:
		# Day time - peak at 0.5 (noon)
		var noon_factor = 1.0 - abs(time_of_day - 0.5) * 4.0
		current_irradiance = solar_irradiance * noon_factor
	
	# Calculate power output
	electricity_production_rate = panel_area * current_irradiance * efficiency
	
	# Call parent for base simulation
	super.simulate(delta)

func _update_display():
	var electricity_bar = get_node_or_null("ElectricityBar")
	if electricity_bar:
		electricity_bar.value = stored_electricity
	
	var status_label = get_node_or_null("StatusLabel")
	if status_label:
		if current_irradiance > 0:
			status_label.text = "Status: Generating"
		else:
			status_label.text = "Status: Night (Idle)"
	
	var daynight_label = get_node_or_null("DayNightLabel")
	if daynight_label:
		if current_irradiance > 0:
			daynight_label.text = "Day/Night: Day ☀️"
		else:
			daynight_label.text = "Day/Night: Night 🌙"
	
	var info_label = get_node_or_null("InfoLabel")
	if info_label:
		info_label.text = "Output: %.1f kW" % electricity_production_rate
	
	# Update visual appearance based on active state
	update_component_status_color(current_irradiance > 0)

func _get_resource_type_from_port(port_index: int) -> int:
	return ResourceType.ELECTRICITY 
