extends Node2D

var loader: MOLoader
var components: Dictionary = {}
@onready var component_area = $ComponentArea

func _ready():
	print("Starting main scene")
	loader = MOLoader.new()
	add_child(loader)
	
	# Load mechanical components
	_load_mechanical_components()
	
	# Load electrical components
	_load_electrical_components()
	
	# Create example connections
	_create_example_circuit()

func _load_mechanical_components():
	print("Loading mechanical components")
	# Load spring
	var spring = loader.load_component("res://apps/modelica_godot/components/mechanical/Spring.mo")
	if spring:
		print("Spring loaded successfully")
		spring.position = Vector2(200, 200)
		component_area.add_child(spring)
		components["spring"] = spring
	else:
		push_error("Failed to load spring component")
	
	# Load mass (once we restore it)
	# var mass = loader.load_component("res://apps/modelica_godot/components/mechanical/Mass.mo")
	# if mass:
	#     mass.position = Vector2(300, 200)
	#     component_area.add_child(mass)
	#     components["mass"] = mass

func _load_electrical_components():
	print("Loading electrical components")
	# Load components from Electrical/Components.mo
	var resistor = _create_component_from_package(
		"res://apps/modelica_godot/components/Electrical/Components.mo",
		"Resistor"
	)
	if resistor:
		resistor.position = Vector2(200, 300)
		component_area.add_child(resistor)
		components["resistor"] = resistor
	
	var voltage_source = _create_component_from_package(
		"res://apps/modelica_godot/components/Electrical/Components.mo",
		"VoltageSource"
	)
	if voltage_source:
		voltage_source.position = Vector2(100, 300)
		component_area.add_child(voltage_source)
		components["voltage_source"] = voltage_source

func _create_component_from_package(package_path: String, component_name: String) -> Node:
	print("Creating component from package: ", package_path, " component: ", component_name)
	# This is a placeholder - we need to implement package parsing
	# For now, we'll just create a basic node with the component name
	var node = Node2D.new()
	node.name = component_name
	
	# Add basic visual representation
	var rect = ColorRect.new()
	rect.size = Vector2(50, 50)
	rect.position = Vector2(-25, -25)
	node.add_child(rect)
	
	var label = Label.new()
	label.text = component_name
	label.position = Vector2(-25, 30)
	node.add_child(label)
	
	return node

func _create_example_circuit():
	# This will be implemented once we have the connection system working
	pass

func _process(_delta):
	# Update component states based on equations
	# This will be implemented once we have the equation solver working
	pass 
