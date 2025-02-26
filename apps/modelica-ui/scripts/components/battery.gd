@tool
extends "res://apps/lunsim/scripts/components/base_component.gd"

class_name Battery

var charge_efficiency: float = 0.95  # 95% charging efficiency
var discharge_efficiency: float = 0.98  # 98% discharging efficiency
var discharge_rate: float = 10.0  # kW maximum discharge rate

func _init():
	super()
	name = "Battery"
	title = "Battery"

func setup_component():
	# Batteries store a large amount of electricity
	max_electricity = 200.0
	max_oxygen = 0.0
	max_water = 0.0
	
	# No consumption or production rates by default
	electricity_production_rate = 0.0
	oxygen_production_rate = 0.0
	water_production_rate = 0.0
	
	electricity_consumption_rate = 0.0
	oxygen_consumption_rate = 0.0
	water_consumption_rate = 0.0
	
	# Add a container to show battery charge
	var vbox = VBoxContainer.new()
	add_child(vbox)
	
	# Use a placeholder emoji instead of preloading the icon
	var placeholder = Label.new()
	placeholder.text = "🔋"
	placeholder.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	placeholder.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	vbox.add_child(placeholder)
	
	var label = Label.new()
	label.name = "StatusLabel"
	label.text = "Status: Empty"
	vbox.add_child(label)
	
	var progress = ProgressBar.new()
	progress.name = "ChargeBar"
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
	info_label.text = "Charge: 0.0 kW"
	vbox.add_child(info_label)

func _setup_slots():
	# Batteries have both input and output for electricity
	set_slot(0, true, ResourceType.ELECTRICITY, _get_slot_color(ResourceType.ELECTRICITY), 
			true, ResourceType.ELECTRICITY, _get_slot_color(ResourceType.ELECTRICITY))

func simulate(delta: float):
	# Batteries have special logic for charging and discharging
	# We override the base behavior completely
	
	# Calculate total power from input connections
	var input_power = 0.0
	for port_index in input_connections.keys():
		var source = input_connections[port_index]
		if source.stored_electricity > 0:
			# Take power from the source (will be limited by transfer rate in parent class)
			input_power += 10.0 * delta  # 10 kW transfer rate
	
	# Apply charging efficiency
	var actual_charge = input_power * charge_efficiency
	
	# Add charge to battery
	stored_electricity += actual_charge
	stored_electricity = min(stored_electricity, max_electricity)
	
	# Handle outgoing connections via the parent class
	_handle_resource_transfers(delta)
	
	# Update display
	_update_display()

func _update_display():
	var charge_bar = get_node_or_null("ChargeBar")
	if charge_bar:
		charge_bar.value = stored_electricity
	
	var status_label = get_node_or_null("StatusLabel")
	if status_label:
		# Calculate percentage of charge
		var charge_percent = (stored_electricity / max_electricity) * 100
		
		if charge_percent <= 10:
			status_label.text = "Status: Critical"
		elif charge_percent <= 25:
			status_label.text = "Status: Low"
		elif charge_percent >= 95:
			status_label.text = "Status: Full"
		else:
			status_label.text = "Status: Charging"
	
	var info_label = get_node_or_null("InfoLabel")
	if info_label:
		info_label.text = "Charge: %.1f kW" % stored_electricity

func _get_resource_type_from_port(port_index: int) -> int:
	return ResourceType.ELECTRICITY 