class_name LCResourceNode
extends RefCounted

## Node in a resource network
##
## Represents a tank or processor that can supply or consume resources.

enum NodeType {
	TANK,      # Storage tank
	PRODUCER,  # Process that produces resources
	CONSUMER   # Process that consumes resources
}

var node_type: NodeType
var effector: Node  # Reference to the actual effector (tank or processor)
var resource_id: String
var connections: Array[LCResourceNode] = []

func _init(type: NodeType, eff: Node, res_id: String):
	node_type = type
	effector = eff
	resource_id = res_id

## Get available resource amount (for suppliers)
func get_available() -> float:
	if effector is LCResourceTankEffector:
		return effector.get_amount()
	return 0.0

## Get space available (for consumers)
func get_space() -> float:
	if effector is LCResourceTankEffector:
		return effector.capacity - effector.get_amount()
	return 0.0

## Request resource (pull model)
func request_resource(amount: float) -> float:
	if effector is LCResourceTankEffector:
		return effector.remove_resource(amount)
	return 0.0

## Supply resource (push model)
func supply_resource(amount: float) -> float:
	if effector is LCResourceTankEffector:
		return effector.add_resource(amount)
	return 0.0

## Check if node is valid
func is_valid() -> bool:
	return is_instance_valid(effector)

## Connect to another node
func connect_to(other: LCResourceNode):
	if other and other.resource_id == resource_id and not connections.has(other):
		connections.append(other)

## Disconnect from another node
func disconnect_from(other: LCResourceNode):
	connections.erase(other)
