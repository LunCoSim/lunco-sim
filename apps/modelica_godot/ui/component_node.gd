class_name ComponentGraphNode
extends GraphNode

var component: ModelicaComponent

func _init(component_: ModelicaComponent):
	component = component_
	title = component.get_class()
	
	# Add ports for each connector
	for connector_name in component.connectors:
		var connector = component.get_connector(connector_name)
		add_port(connector_name, connector.type)

func add_port(name: String, type: ModelicaConnector.Type) -> void:
	# Add input/output slot
	set_slot(get_child_count(), true, type, Color.BLUE,
			 true, type, Color.RED)
	
	# Add label for port
	var label = Label.new()
	label.text = name
	add_child(label)

func update_visualization() -> void:
	# Update visual representation based on component state
	# This will be overridden by specific component nodes
	pass 
