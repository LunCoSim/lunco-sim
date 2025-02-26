@tool
extends GraphNode

class_name BaseComponent

# Resource storage
var stored_electricity: float = 0
var stored_oxygen: float = 0
var stored_water: float = 0

# Resource capacity
var max_electricity: float = 100
var max_oxygen: float = 100
var max_water: float = 100

# Input/output connections
var input_connections = {}
var output_connections = {}

# Resource port types
enum ResourceType {
	ELECTRICITY,
	OXYGEN, 
	WATER
}

# Resource transfer rates
var electricity_production_rate: float = 0  # kW
var oxygen_production_rate: float = 0  # m³/s
var water_production_rate: float = 0  # L/s

var electricity_consumption_rate: float = 0  # kW
var oxygen_consumption_rate: float = 0  # m³/s
var water_consumption_rate: float = 0  # L/s

func _init():
	# Configure the GraphNode
	title = "Base Component"
	resizable = true
	selectable = true
	
	# Set default size
	size = Vector2(200, 150)
	
	# Connect signals
	# We'll connect delete_request in _ready() instead since _init may be too early
	
	# Initial setup
	setup_component()

func _ready():
	# Connect close button signal
	delete_request.connect(_on_close_request)
	
	# Add input and output slot labels
	_setup_slots()

func setup_component():
	# Override in child classes to configure component properties
	pass

func simulate(delta: float):
	# Base simulation step - override in child classes
	
	# Produce resources
	stored_electricity += electricity_production_rate * delta
	stored_oxygen += oxygen_production_rate * delta
	stored_water += water_production_rate * delta
	
	# Consume resources
	stored_electricity -= electricity_consumption_rate * delta
	stored_oxygen -= oxygen_consumption_rate * delta
	stored_water -= water_consumption_rate * delta
	
	# Clamp values to limits
	stored_electricity = clamp(stored_electricity, 0, max_electricity)
	stored_oxygen = clamp(stored_oxygen, 0, max_oxygen)
	stored_water = clamp(stored_water, 0, max_water)
	
	# Handle resource transfers
	_handle_resource_transfers(delta)
	
	# Update visual display
	_update_display()

func connect_input(source_component, port_index: int):
	input_connections[port_index] = source_component

func disconnect_input(source_component, port_index: int):
	if input_connections.has(port_index) and input_connections[port_index] == source_component:
		input_connections.erase(port_index)

func connect_output(target_component, port_index: int):
	if not output_connections.has(port_index):
		output_connections[port_index] = []
	
	if not output_connections[port_index].has(target_component):
		output_connections[port_index].append(target_component)

func disconnect_output(target_component, port_index: int):
	if output_connections.has(port_index) and output_connections[port_index].has(target_component):
		output_connections[port_index].erase(target_component)

func _setup_slots():
	# Set up the slots for connecting components
	# Left side (inputs) / Right side (outputs)
	# Override in child classes
	pass

func _get_slot_color(resource_type: int) -> Color:
	match resource_type:
		ResourceType.ELECTRICITY:
			return Color(1, 1, 0)  # Yellow for electricity
		ResourceType.OXYGEN:
			return Color(0, 0.8, 1)  # Blue for oxygen
		ResourceType.WATER:
			return Color(0, 0.5, 1)  # Darker blue for water
		_:
			return Color(0.7, 0.7, 0.7)  # Gray for unknown

# Add helpful tooltip text to explain component functionality
func set_component_tooltip(text: String):
	tooltip_text = text
	
# Add resource type labels to slots to make connections clearer
func add_slot_labels(container: Control, input_types: Array = [], output_types: Array = []):
	# Create a horizontal container for input and output labels
	var hbox = HBoxContainer.new()
	hbox.size_flags_horizontal = Control.SIZE_FILL
	container.add_child(hbox)
	
	# Left side for inputs
	var left_vbox = VBoxContainer.new()
	left_vbox.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	hbox.add_child(left_vbox)
	
	# Add input labels
	for type in input_types:
		var label = Label.new()
		label.text = _get_resource_name(type) + " ◄"
		label.horizontal_alignment = HORIZONTAL_ALIGNMENT_LEFT
		left_vbox.add_child(label)
	
	# Spacer in the middle
	var spacer = Control.new()
	spacer.custom_minimum_size = Vector2(20, 0)
	hbox.add_child(spacer)
	
	# Right side for outputs
	var right_vbox = VBoxContainer.new()
	right_vbox.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	hbox.add_child(right_vbox)
	
	# Add output labels
	for type in output_types:
		var label = Label.new()
		label.text = "► " + _get_resource_name(type)
		label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
		right_vbox.add_child(label)

# Get human-readable resource name
func _get_resource_name(resource_type: int) -> String:
	match resource_type:
		ResourceType.ELECTRICITY:
			return "Power"
		ResourceType.OXYGEN:
			return "Oxygen"
		ResourceType.WATER:
			return "Water"
		_:
			return "Unknown"

# Visual indication of component status
func update_component_status_color(is_active: bool):
	if is_active:
		self.modulate = Color(1, 1, 1)  # Normal color when active
	else:
		self.modulate = Color(0.7, 0.7, 0.7)  # Grayed out when inactive

func _handle_resource_transfers(delta: float):
	# Transfer resources through connections
	# This is a simplified version - in a real simulation this would be more complex
	for port_index in output_connections.keys():
		var resource_type = _get_resource_type_from_port(port_index)
		var targets = output_connections[port_index]
		
		# Skip if no targets
		if targets.size() == 0:
			continue
		
		# Determine how much to transfer
		var transfer_amount = 0
		match resource_type:
			ResourceType.ELECTRICITY:
				# Base transfer on excess electricity
				transfer_amount = min(stored_electricity, 10 * delta)
			ResourceType.OXYGEN:
				transfer_amount = min(stored_oxygen, 5 * delta)
			ResourceType.WATER:
				transfer_amount = min(stored_water, 5 * delta)
		
		# Divide evenly among targets
		var amount_per_target = transfer_amount / targets.size()
		
		# Transfer to each target
		for target in targets:
			match resource_type:
				ResourceType.ELECTRICITY:
					var actual_transfer = min(amount_per_target, target.max_electricity - target.stored_electricity)
					stored_electricity -= actual_transfer
					target.stored_electricity += actual_transfer
				ResourceType.OXYGEN:
					var actual_transfer = min(amount_per_target, target.max_oxygen - target.stored_oxygen)
					stored_oxygen -= actual_transfer
					target.stored_oxygen += actual_transfer
				ResourceType.WATER:
					var actual_transfer = min(amount_per_target, target.max_water - target.stored_water)
					stored_water -= actual_transfer
					target.stored_water += actual_transfer

func _get_resource_type_from_port(port_index: int) -> int:
	# Override in child classes to define port resource types
	return ResourceType.ELECTRICITY

func _update_display():
	# Update visual elements based on resource levels
	# Override in child classes
	pass

func _on_close_request():
	# Disconnect all connections before removing
	for port_index in input_connections.keys():
		var source = input_connections[port_index]
		get_parent().disconnect_node(source.name, port_index, name, port_index)
	
	for port_index in output_connections.keys():
		for target in output_connections[port_index]:
			get_parent().disconnect_node(name, port_index, target.name, port_index)
	
	# Remove from parent's component list
	var parent = get_parent().get_parent()
	if parent.has_method("remove_component"):
		parent.remove_component(self)
	
	# Delete the component
	queue_free() 