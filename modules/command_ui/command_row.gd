extends PanelContainer

## A single command row in the UI with dynamic parameter inputs.

@onready var cmd_name_label = %CommandName
@onready var execute_button = %ExecuteButton
@onready var param_container = %ParamContainer

var target_path: String = ""
var command_info: Dictionary = {}
var param_controls: Dictionary = {}

func _ready():
	execute_button.pressed.connect(_on_execute_pressed)

func setup(target: String, info: Dictionary):
	target_path = target
	command_info = info
	
	cmd_name_label.text = info.name
	
	_build_params()

func _build_params():
	# Clear existing
	for child in param_container.get_children():
		child.queue_free()
	
	param_controls.clear()
	
	var args = command_info.get("arguments", [])
	if args.is_empty():
		param_container.hide()
		return
	
	param_container.show()
	for arg in args:
		_create_param_input(arg)

func _create_param_input(arg: Dictionary):
	var row = HBoxContainer.new()
	param_container.add_child(row)
	
	var label = Label.new()
	label.text = arg.name.capitalize() + ":"
	label.custom_minimum_size.x = 80
	row.add_child(label)
	
	var control: Control = null
	var type = arg.get("type", "String")
	
	match type:
		"float", "int", "float", "Real":
			var slider = HSlider.new()
			slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
			slider.min_value = arg.get("min", 0.0)
			slider.max_value = arg.get("max", 100.0)
			slider.step = arg.get("step", 0.01 if "float" in type.to_lower() else 1.0)
			slider.value = arg.get("default", 0.0)
			
			var val_label = Label.new()
			val_label.text = str(slider.value)
			val_label.custom_minimum_size.x = 40
			
			slider.value_changed.connect(func(v): val_label.text = "%.2f" % v if "float" in type.to_lower() else str(int(v)))
			
			row.add_child(slider)
			row.add_child(val_label)
			control = slider
		"bool", "bool":
			var checkbox = CheckBox.new()
			checkbox.button_pressed = arg.get("default", false)
			row.add_child(checkbox)
			control = checkbox
		_:
			var line_edit = LineEdit.new()
			line_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
			line_edit.text = str(arg.get("default", ""))
			row.add_child(line_edit)
			control = line_edit
			
	param_controls[arg.name] = control

func _on_execute_pressed():
	var args = {}
	for arg_name in param_controls:
		var ctrl = param_controls[arg_name]
		if ctrl is HSlider:
			args[arg_name] = ctrl.value
		elif ctrl is CheckBox:
			args[arg_name] = ctrl.button_pressed
		elif ctrl is LineEdit:
			args[arg_name] = ctrl.text
	
	var cmd = LCCommand.new(command_info.name, NodePath(target_path), args, "ui_dashboard")
	var result = LCCommandRouter.dispatch(cmd)
	print("Command Result: ", result)
