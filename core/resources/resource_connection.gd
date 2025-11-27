class_name LCResourceConnection
extends RefCounted

## Connection between two resource nodes
##
## Manages resource flow between nodes based on flow rules.

var from_node: LCResourceNode
var to_node: LCResourceNode
var max_flow_rate: float = 10.0  # kg/s or units/s
var is_active: bool = true
var total_transferred: float = 0.0

func _init(from: LCResourceNode, to: LCResourceNode, flow_rate: float = 10.0):
	from_node = from
	to_node = to
	max_flow_rate = flow_rate

## Transfer resources based on delta time
func transfer(delta: float) -> float:
	if not is_active or not from_node or not to_node:
		return 0.0
	
	if not from_node.is_valid() or not to_node.is_valid():
		return 0.0
	
	# Calculate how much can be transferred
	var available = from_node.get_available()
	var space = to_node.get_space()
	var max_transfer = max_flow_rate * delta
	
	var transfer_amount = min(available, min(space, max_transfer))
	
	if transfer_amount > 0:
		var removed = from_node.request_resource(transfer_amount)
		var added = to_node.supply_resource(removed)
		total_transferred += added
		return added
	
	return 0.0

## Check if connection is valid
func is_valid() -> bool:
	return from_node and to_node and from_node.is_valid() and to_node.is_valid()
