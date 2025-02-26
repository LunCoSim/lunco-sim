@tool
extends "res://apps/lunsim/scripts/components/base_component.gd"

class_name Habitat

var crew_count: int = 4  # Number of people in the habitat
var is_active: bool = true

func _init():
	super()
	name = "Habitat"
	title = "Habitat Module"

func setup_component():
	# Habitats need all resources
	max_electricity = 50.0
	max_oxygen = 100.0
	max_water = 100.0
	
	# Set consumption rates based on crew
	electricity_consumption_rate = 2.0 * crew_count  # 2 kW per person
	oxygen_consumption_rate = 0.5 * crew_count  # 0.5 m³ per person
	water_consumption_rate = 0.3 * crew_count  # 0.3 L per person
	
	# Habitats don't produce resources
	electricity_production_rate = 0.0
	oxygen_production_rate = 0.0
	water_production_rate = 0.0
	
	# Add a container for UI elements
	var vbox = VBoxContainer.new()
	add_child(vbox)
	
	# Use a placeholder emoji instead of preloading the icon
	var placeholder = Label.new()
	placeholder.text = "🏠"
	placeholder.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	placeholder.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	vbox.add_child(placeholder)
	
	# Status label
	var status_label = Label.new()
	status_label.name = "StatusLabel"
	status_label.text = "Status: Active"
	vbox.add_child(status_label)
	
	# Crew label
	var crew_label = Label.new()
	crew_label.name = "CrewLabel"
	crew_label.text = "Crew: %d" % crew_count
	vbox.add_child(crew_label)
	
	# Resource bars
	var hbox = HBoxContainer.new()
	vbox.add_child(hbox)
	
	# Power display
	var power_vbox = VBoxContainer.new()
	hbox.add_child(power_vbox)
	
	var power_label = Label.new()
	power_label.text = "Power"
	power_vbox.add_child(power_label)
	
	var power_bar = ProgressBar.new()
	power_bar.name = "PowerBar"
	power_bar.max_value = max_electricity
	power_bar.value = 0
	power_bar.show_percentage = false
	power_vbox.add_child(power_bar)
	
	# Oxygen display
	var oxygen_vbox = VBoxContainer.new()
	hbox.add_child(oxygen_vbox)
	
	var oxygen_label = Label.new()
	oxygen_label.text = "O₂"
	oxygen_vbox.add_child(oxygen_label)
	
	var oxygen_bar = ProgressBar.new()
	oxygen_bar.name = "OxygenBar"
	oxygen_bar.max_value = max_oxygen
	oxygen_bar.value = 0
	oxygen_bar.show_percentage = false
	oxygen_vbox.add_child(oxygen_bar)
	
	# Water display
	var water_vbox = VBoxContainer.new()
	hbox.add_child(water_vbox)
	
	var water_label = Label.new()
	water_label.text = "H₂O"
	water_vbox.add_child(water_label)
	
	var water_bar = ProgressBar.new()
	water_bar.name = "WaterBar"
	water_bar.max_value = max_water
	water_bar.value = 0
	water_bar.show_percentage = false
	water_vbox.add_child(water_bar)
	
	# Info label for resource consumption
	var info_label = Label.new()
	info_label.name = "InfoLabel"
	info_label.text = "Consumption:\nPower: %.1f kW\nO₂: %.1f m³/s\nH₂O: %.1f L/s" % [
		electricity_consumption_rate,
		oxygen_consumption_rate,
		water_consumption_rate
	]
	vbox.add_child(info_label)

func _setup_slots():
	# Habitats receive all resource types but don't output any
	set_slot(0, true, ResourceType.ELECTRICITY, _get_slot_color(ResourceType.ELECTRICITY), 
			false, ResourceType.ELECTRICITY, Color(0, 0, 0))
	set_slot(1, true, ResourceType.OXYGEN, _get_slot_color(ResourceType.OXYGEN), 
			false, ResourceType.OXYGEN, Color(0, 0, 0))
	set_slot(2, true, ResourceType.WATER, _get_slot_color(ResourceType.WATER), 
			false, ResourceType.WATER, Color(0, 0, 0))

func simulate(delta: float):
	# Check if we have enough resources to stay active
	var power_status = stored_electricity >= electricity_consumption_rate * delta
	var oxygen_status = stored_oxygen >= oxygen_consumption_rate * delta
	var water_status = stored_water >= water_consumption_rate * delta
	
	is_active = power_status and oxygen_status and water_status
	
	# Only consume resources if active
	if is_active:
		super.simulate(delta)
	else:
		# Update display without consuming resources
		_update_display()

func _update_display():
	# Update status label
	var status_label = get_node_or_null("StatusLabel")
	if status_label:
		if is_active:
			status_label.text = "Status: Active"
		else:
			# Determine what resource is missing
			var power_status = stored_electricity >= electricity_consumption_rate * 0.1  # 0.1 seconds worth
			var oxygen_status = stored_oxygen >= oxygen_consumption_rate * 0.1
			var water_status = stored_water >= water_consumption_rate * 0.1
			
			if not power_status:
				status_label.text = "Status: Power Critical!"
			elif not oxygen_status:
				status_label.text = "Status: Oxygen Critical!"
			elif not water_status:
				status_label.text = "Status: Water Critical!"
			else:
				status_label.text = "Status: Resources Low"
	
	# Update progress bars
	var power_bar = get_node_or_null("PowerBar")
	if power_bar:
		power_bar.value = stored_electricity
	
	var oxygen_bar = get_node_or_null("OxygenBar")
	if oxygen_bar:
		oxygen_bar.value = stored_oxygen
	
	var water_bar = get_node_or_null("WaterBar")
	if water_bar:
		water_bar.value = stored_water

func _get_resource_type_from_port(port_index: int) -> int:
	match port_index:
		0:
			return ResourceType.ELECTRICITY
		1:
			return ResourceType.OXYGEN
		2:
			return ResourceType.WATER
		_:
			return ResourceType.ELECTRICITY 