class_name LCEffectorPanel
extends PanelContainer

## UI panel for a single effector.
## Displays telemetry and allows control.

var effector: Node
var telemetry_grid: GridContainer
var controls_container: VBoxContainer

func setup(target_effector: Node):
	effector = target_effector
	name = effector.name + "Panel"
	
	var vbox = VBoxContainer.new()
	add_child(vbox)
	
	# Header
	var header = Label.new()
	header.text = effector.name
	header.add_theme_font_size_override("font_size", 16)
	header.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	vbox.add_child(header)
	
	var type_label = Label.new()
	if effector is LCStateEffector:
		type_label.text = "State Effector"
	elif effector is LCDynamicEffector:
		type_label.text = "Dynamic Effector"
	elif effector is LCSensorEffector:
		type_label.text = "Sensor"
	type_label.add_theme_color_override("font_color", Color(0.7, 0.7, 0.7))
	type_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	vbox.add_child(type_label)
	
	vbox.add_child(HSeparator.new())
	
	# Telemetry Section
	var tel_label = Label.new()
	tel_label.text = "Telemetry"
	tel_label.add_theme_stylebox_override("normal", StyleBoxFlat.new())
	vbox.add_child(tel_label)
	
	telemetry_grid = GridContainer.new()
	telemetry_grid.columns = 2
	vbox.add_child(telemetry_grid)
	
	vbox.add_child(HSeparator.new())
	
	# Controls Section
	var ctrl_label = Label.new()
	ctrl_label.text = "Controls"
	vbox.add_child(ctrl_label)
	
	controls_container = VBoxContainer.new()
	vbox.add_child(controls_container)
	
	_create_controls()

func _process(delta):
	if not is_instance_valid(effector):
		return
		
	_update_telemetry()

func _update_telemetry():
	# Clear existing telemetry
	for child in telemetry_grid.get_children():
		child.queue_free()
	
	if effector.has_method("get_telemetry"):
		var data = effector.get_telemetry()
		for key in data:
			var label = Label.new()
			label.text = str(key) + ":"
			label.add_theme_color_override("font_color", Color(0.8, 0.8, 0.8))
			telemetry_grid.add_child(label)
			
			var value = Label.new()
			var val = data[key]
			if val is float:
				value.text = "%.3f" % val
			elif val is Vector3:
				value.text = "(%.2f, %.2f, %.2f)" % [val.x, val.y, val.z]
			else:
				value.text = str(val)
			telemetry_grid.add_child(value)

func _create_controls():
	# Create controls based on effector type
	if effector is LCThrusterEffector:
		_add_slider("Thrust", 0.0, 1.0, func(val): effector.set_thrust(val))
		if effector.can_vector:
			_add_slider("Gimbal Pitch", -5.0, 5.0, func(val): effector.set_gimbal(val, effector.current_gimbal.y))
			_add_slider("Gimbal Yaw", -5.0, 5.0, func(val): effector.set_gimbal(effector.current_gimbal.x, val))
			
	elif effector is LCReactionWheelEffector:
		_add_slider("Torque", -effector.max_torque, effector.max_torque, func(val): effector.set_torque(val))
		_add_button("Dump Momentum", func(): effector.dump_momentum(0.1, 1.0))
		
	elif effector is LCSolarPanelEffector:
		if effector.is_deployable:
			_add_button("Deploy", func(): effector.deploy())
			_add_button("Stow", func(): effector.stow())
		if effector.can_articulate:
			_add_button("Track Sun", func(): effector.enable_sun_tracking())
			
	elif effector is LCBatteryEffector:
		# Battery is mostly passive but we could add debug controls
		pass
		
	elif effector is LCLidarEffector:
		_add_checkbox("Enabled", effector.is_enabled, func(val): effector.is_enabled = val)
		
	elif effector is LCCameraEffector:
		_add_checkbox("Enabled", effector.is_enabled, func(val): effector.is_enabled = val)
		_add_checkbox("Object Det.", effector.enable_object_detection, func(val): effector.enable_object_detection = val)

func _add_slider(label_text: String, min_val: float, max_val: float, callback: Callable):
	var hbox = HBoxContainer.new()
	var label = Label.new()
	label.text = label_text
	label.custom_minimum_size.x = 100
	hbox.add_child(label)
	
	var slider = HSlider.new()
	slider.min_value = min_val
	slider.max_value = max_val
	slider.step = (max_val - min_val) / 100.0
	slider.size_flags_horizontal = SIZE_EXPAND_FILL
	slider.value_changed.connect(callback)
	hbox.add_child(slider)
	
	controls_container.add_child(hbox)

func _add_button(text: String, callback: Callable):
	var btn = Button.new()
	btn.text = text
	btn.pressed.connect(callback)
	controls_container.add_child(btn)

func _add_checkbox(text: String, initial: bool, callback: Callable):
	var cb = CheckBox.new()
	cb.text = text
	cb.button_pressed = initial
	cb.toggled.connect(callback)
	controls_container.add_child(cb)
