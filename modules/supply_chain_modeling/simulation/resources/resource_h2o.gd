class_name ResourceH2O
extends BaseResource

func _init():
	mass = 18.015  # g/mol
	volume = 0.0  # Placeholder value
	set_properties("Water", "product", mass, volume)

func save_state() -> Dictionary:
	var state = super.save_state()
	
	
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
