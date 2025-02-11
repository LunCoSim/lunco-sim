class_name NoteNode
extends SimulationNode

@export var note_text: String = ""

func save_state() -> Dictionary:
	var state = super.save_state()
	
	state["note_text"] = note_text
	
	return state

func load_state(state: Dictionary) -> void:
	if state:
		note_text = state.get("note_text", "")

	super.load_state(state)
