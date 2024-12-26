extends VBoxContainer

signal button_down(path: String)
signal button_up()

func _ready() -> void:
	create_buttons()

func create_buttons() -> void:

	var class_map = Utils.class_to_script_map

	for custom_class_name in class_map:
		var button = Button.new()
		button.text = custom_class_name
		button.connect("button_down", func(): button_down.emit(custom_class_name))
		button.connect("button_up", func(): button_up.emit())
		add_child(button)
	