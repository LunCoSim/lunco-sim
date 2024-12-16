class_name StorageFacility
extends BaseFacility

@export var capacity: float = 100.0  # Maximum storage capacity
@export var current_amount: float = 0.0  # Current amount stored
@export var resource_type: String = ""  # Type of resource being stored

func _init() -> void:
	pass
	
