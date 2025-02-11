extends VBoxContainer

signal button_down(path: String)
signal button_up()

var Utils

func _ready() -> void:
	# Get Utils from parent scene
	await get_tree().create_timer(0.1).timeout  # Wait for parent scene to initialize
	var root = get_tree().root.get_node("RSCT")
	if root:
		Utils = root.Utils
		create_buttons()

func create_buttons() -> void:
	if !Utils:
		return
		
	var class_map = Utils.class_to_script_map

	for custom_class_name in class_map:
		var button = Button.new()
		button.text = custom_class_name
		button.connect("button_down", func(): button_down.emit(custom_class_name))
		button.connect("button_up", func(): button_up.emit())
		add_child(button)
	