class_name BaseFacility
extends SimulationNode

# Basic facility properties as defined in SPECIFICATION.md
@export var facility_id: String
@export var description: String
@export var facility_type: String  # producer, consumer, storage, or custom
@export var efficiency: float = 1.0
@export var status: String = "Running"
var custom_properties: Dictionary = {}
var metadata: Dictionary = {}

func process_step(delta: float) -> void:
	pass
	#var inputs = get_input_resources(delta)
	#if can_process(inputs):
		#consume_resources(inputs)
		#produce_outputs(delta)
