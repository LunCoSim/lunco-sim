extends Control

# Time multipliers for simulation speed
const TIME_PAUSED = 0
const TIME_NORMAL = 1
const TIME_FAST = 2
const TIME_VERY_FAST = 5

# Current time multiplier
var time_multiplier = TIME_NORMAL

# Resource tracking
var resources = {
	"electricity": 0,
	"oxygen": 0,
	"water": 0
}

# References to UI elements
@onready var simulation_area = $SimulationArea
@onready var component_list = $UI/ComponentPanel/VBoxContainer/ScrollContainer/ComponentList
@onready var electricity_label = $UI/ResourceDisplay/VBoxContainer/ElectricityLabel
@onready var oxygen_label = $UI/ResourceDisplay/VBoxContainer/OxygenLabel
@onready var water_label = $UI/ResourceDisplay/VBoxContainer/WaterLabel

# Track all placed components
var components = []

func _ready():
	# Connect time control buttons
	$UI/TimeControls/HBoxContainer/PauseButton.pressed.connect(_on_pause_button_pressed)
	$UI/TimeControls/HBoxContainer/NormalSpeedButton.pressed.connect(_on_normal_speed_button_pressed)
	$UI/TimeControls/HBoxContainer/FastSpeedButton.pressed.connect(_on_fast_speed_button_pressed)
	$UI/TimeControls/HBoxContainer/VeryFastSpeedButton.pressed.connect(_on_very_fast_speed_button_pressed)
	
	# Connect GraphEdit signals
	simulation_area.connection_request.connect(_on_connection_request)
	simulation_area.disconnection_request.connect(_on_disconnection_request)
	
	# Populate component list
	_populate_component_list()

func _process(delta):
	# Skip simulation steps when paused
	if time_multiplier == TIME_PAUSED:
		return
		
	# Apply time multiplier to delta time
	var simulation_delta = delta * time_multiplier
	
	# Simulate all components
	for component in components:
		component.simulate(simulation_delta)
	
	# Update resource displays
	_update_resource_display()

func _populate_component_list():
	# Create buttons for each component type
	_add_component_button("Solar Panel", "solar_panel")
	_add_component_button("Battery", "battery")
	_add_component_button("Habitat Module", "habitat")
	_add_component_button("Oxygen Generator", "oxygen_generator")
	_add_component_button("Water Recycler", "water_recycler")

func _add_component_button(name, type):
	var button = Button.new()
	button.text = name
	button.size_flags_horizontal = Control.SIZE_FILL
	button.pressed.connect(func(): _on_component_button_pressed(type))
	component_list.add_child(button)

func _on_component_button_pressed(type):
	# Create the appropriate component based on type
	var component
	match type:
		"solar_panel":
			component = preload("res://apps/lunsim/scripts/components/solar_panel.gd").new()
		"battery":
			component = preload("res://apps/lunsim/scripts/components/battery.gd").new()
		"habitat":
			component = preload("res://apps/lunsim/scripts/components/habitat.gd").new()
		"oxygen_generator":
			component = preload("res://apps/lunsim/scripts/components/oxygen_generator.gd").new()
		"water_recycler":
			component = preload("res://apps/lunsim/scripts/components/water_recycler.gd").new()
	
	# Add component to simulation area
	simulation_area.add_child(component)
	components.append(component)

func _on_connection_request(from_node, from_port, to_node, to_port):
	# Connect components
	simulation_area.connect_node(from_node, from_port, to_node, to_port)
	
	# Get the component instances - use simulation_area as the parent and convert string to NodePath
	var from_component = simulation_area.get_node(NodePath(from_node))
	var to_component = simulation_area.get_node(NodePath(to_node))
	
	# Set up resource connections
	from_component.connect_output(to_component, from_port)
	to_component.connect_input(from_component, to_port)

func _on_disconnection_request(from_node, from_port, to_node, to_port):
	# Disconnect components
	simulation_area.disconnect_node(from_node, from_port, to_node, to_port)
	
	# Get the component instances - use simulation_area as the parent and convert string to NodePath
	var from_component = simulation_area.get_node(NodePath(from_node))
	var to_component = simulation_area.get_node(NodePath(to_node))
	
	# Remove resource connections
	from_component.disconnect_output(to_component, from_port)
	to_component.disconnect_input(from_component, to_port)

func _update_resource_display():
	# Calculate total resources from all components
	var total_electricity = 0
	var total_oxygen = 0
	var total_water = 0
	
	for component in components:
		total_electricity += component.stored_electricity
		total_oxygen += component.stored_oxygen
		total_water += component.stored_water
	
	# Update UI
	electricity_label.text = "Electricity: %.1f kW" % total_electricity
	oxygen_label.text = "Oxygen: %.1f m³" % total_oxygen
	water_label.text = "Water: %.1f L" % total_water

# Function to remove a component from the simulation
func remove_component(component):
	if components.has(component):
		components.erase(component)

# Time control functions
func _on_pause_button_pressed():
	time_multiplier = TIME_PAUSED

func _on_normal_speed_button_pressed():
	time_multiplier = TIME_NORMAL

func _on_fast_speed_button_pressed():
	time_multiplier = TIME_FAST

func _on_very_fast_speed_button_pressed():
	time_multiplier = TIME_VERY_FAST 
