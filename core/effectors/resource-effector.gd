class_name LCResourceEffector
extends LCStateEffector

## Interface for components that store and manage resources.
##
## Provides a common API for tanks, batteries (if treated as charge containers),
## and other resource storage components, allowing process effectors 
## to interact with them without sibling dependency cycles.

func get_amount() -> float: return 0.0
func get_fill_percentage() -> float: return 0.0
func set_amount(_new_amount: float): pass
func add_resource(_amount: float): pass
func remove_resource(_amount: float) -> float: return 0.0
func is_empty() -> bool: return true
func is_full() -> bool: return false
func get_resource_name() -> String: return "Unknown"
func get_resource_id() -> String: return ""
