class_name ResourceH2
extends BaseResource

func _init():
	mass = 2.016  # g/mol
	volume = 0.0  # Placeholder value
	set_properties("Hydrogen", "product", mass, volume)

func save_state() -> Dictionary:
	var state = super.save_state()
	
	
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
