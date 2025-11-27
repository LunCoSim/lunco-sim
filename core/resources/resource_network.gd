class_name LCResourceNetwork
extends Node

## Resource flow network
##
## Manages automatic resource flow between connected tanks and processors.
## Solves flow equations and balances supply/demand.

var nodes: Array[LCResourceNode] = []
var connections: Array[LCResourceConnection] = []
var resource_types: Dictionary = {}  # resource_id -> Array[LCResourceNode]

@export var update_rate: float = 0.1  # Update network every 0.1 seconds
var accumulator: float = 0.0

signal network_updated()

func _physics_process(delta: float):
	accumulator += delta
	
	if accumulator >= update_rate:
		_update_network(accumulator)
		accumulator = 0.0

## Add a node to the network
func add_node(effector: Node, node_type: LCResourceNode.NodeType = LCResourceNode.NodeType.TANK) -> LCResourceNode:
	var resource_id = ""
	
	# Determine resource ID
	if effector is LCResourceTankEffector:
		resource_id = effector.resource_id
	elif effector is LCProcessEffector:
		# For processes, we'll create nodes for each input/output
		# This is handled separately
		return null
	
	if resource_id.is_empty():
		return null
	
	# Create node
	var node = LCResourceNode.new(node_type, effector, resource_id)
	nodes.append(node)
	
	# Add to resource type index
	if not resource_types.has(resource_id):
		resource_types[resource_id] = []
	resource_types[resource_id].append(node)
	
	return node

## Add a process effector (creates nodes for inputs/outputs)
func add_process(process: LCProcessEffector):
	if not process or not process.recipe:
		return
	
	# Create consumer nodes for inputs
	for ingredient in process.recipe.input_resources:
		var node = LCResourceNode.new(LCResourceNode.NodeType.CONSUMER, process, ingredient.resource_id)
		nodes.append(node)
		
		if not resource_types.has(ingredient.resource_id):
			resource_types[ingredient.resource_id] = []
		resource_types[ingredient.resource_id].append(node)
	
	# Create producer nodes for outputs
	for product in process.recipe.output_resources:
		var node = LCResourceNode.new(LCResourceNode.NodeType.PRODUCER, product.resource_id, product.resource_id)
		nodes.append(node)
		
		if not resource_types.has(product.resource_id):
			resource_types[product.resource_id] = []
		resource_types[product.resource_id].append(node)

## Connect two nodes
func connect_nodes(from: LCResourceNode, to: LCResourceNode, flow_rate: float = 10.0) -> LCResourceConnection:
	if not from or not to:
		return null
	
	if from.resource_id != to.resource_id:
		push_error("Cannot connect nodes with different resource types")
		return null
	
	var connection = LCResourceConnection.new(from, to, flow_rate)
	connections.append(connection)
	from.connect_to(to)
	
	return connection

## Auto-connect all compatible nodes
func auto_connect():
	# For each resource type, connect all suppliers to all consumers
	for resource_id in resource_types.keys():
		var nodes_of_type = resource_types[resource_id]
		
		var suppliers = []
		var consumers = []
		
		for node in nodes_of_type:
			if node.node_type == LCResourceNode.NodeType.TANK or node.node_type == LCResourceNode.NodeType.PRODUCER:
				suppliers.append(node)
			if node.node_type == LCResourceNode.NodeType.TANK or node.node_type == LCResourceNode.NodeType.CONSUMER:
				consumers.append(node)
		
		# Connect each supplier to each consumer
		for supplier in suppliers:
			for consumer in consumers:
				if supplier != consumer:
					connect_nodes(supplier, consumer)

## Update network flow
func _update_network(delta: float):
	# Remove invalid connections
	connections = connections.filter(func(conn): return conn.is_valid())
	
	# Transfer resources through connections
	for connection in connections:
		connection.transfer(delta)
	
	network_updated.emit()

## Get total amount of a resource in the network
func get_total_resource(resource_id: String) -> float:
	var total = 0.0
	
	if resource_types.has(resource_id):
		for node in resource_types[resource_id]:
			if node.node_type == LCResourceNode.NodeType.TANK:
				total += node.get_available()
	
	return total

## Get total capacity for a resource in the network
func get_total_capacity(resource_id: String) -> float:
	var total = 0.0
	
	if resource_types.has(resource_id):
		for node in resource_types[resource_id]:
			if node.node_type == LCResourceNode.NodeType.TANK and node.effector is LCResourceTankEffector:
				total += node.effector.capacity
	
	return total

## Clear the network
func clear():
	nodes.clear()
	connections.clear()
	resource_types.clear()

## Rebuild network from vehicle
func rebuild_from_vehicle(vehicle: Node):
	clear()
	
	# Find all tanks
	for child in vehicle.get_children():
		if child is LCResourceTankEffector:
			add_node(child)
		elif child is LCProcessEffector:
			add_process(child)
	
	# Auto-connect
	auto_connect()
	
	print("ResourceNetwork: Rebuilt with ", nodes.size(), " nodes and ", connections.size(), " connections")
