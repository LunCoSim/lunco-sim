class_name UINoteNode
extends UISimulationNode

@export var note_text: String = ""

func _ready() -> void:
	super._ready()
	var text_edit = $TextEdit
	text_edit.text = note_text
	text_edit.connect("text_changed", _on_text_changed)

func _on_text_changed() -> void:
	if simulation_node:
		simulation_node.note_text = $TextEdit.text

func update_from_simulation() -> void:
	super.update_from_simulation()
	if simulation_node and simulation_node is NoteNode:
		$TextEdit.text = simulation_node.note_text
