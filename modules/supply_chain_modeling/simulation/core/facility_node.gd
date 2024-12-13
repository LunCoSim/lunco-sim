class_name FacilityNode
extends SimulationNode

func process_step(delta: float) -> void:
    var inputs = get_input_resources(delta)
    if can_process(inputs):
        consume_resources(inputs)
        produce_outputs(delta)