extends VBoxContainer

signal button_down(path: String)
signal button_up()

func _ready() -> void:
	create_buttons()

func create_buttons() -> void:
	var resource_paths = Utils.get_script_paths("res://simulation/resources/")
	var facility_paths = Utils.get_script_paths("res://simulation/facilities/")
	
	for path in resource_paths + facility_paths:
		var button = Button.new()
		button.text = path.get_file().get_basename()
		button.connect("button_down", func(): button_down.emit(path))
		button.connect("button_up", func(): button_up.emit())
		add_child(button)
