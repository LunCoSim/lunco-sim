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

var telemetry_labels: Dictionary = {}
var update_timer: float = 0.0

func _process(delta):
	if not is_instance_valid(effector):
		return
	
	# Throttle updates to 10Hz
	update_timer += delta
	if update_timer > 0.1:
		update_timer = 0.0
		_update_telemetry()

func _update_telemetry():
	var data = {}
	
	# Method 1: Use Telemetry schema if present (preferred)
	var telemetry_schema = effector.get("Telemetry")
	if telemetry_schema is Dictionary:
		for key in telemetry_schema:
			var prop_name = telemetry_schema[key]
			var val = effector.get(prop_name)
			if val != null:
				data[key] = val
				
	# Method 2: Fallback to get_telemetry()
	elif effector.has_method("get_telemetry"):
		data = effector.get_telemetry()
	
	if data.is_empty():
		return
		
	# Remove keys that are no longer present
	var current_keys = data.keys()
	var keys_to_remove = []
	for key in telemetry_labels:
		if not key in data:
			keys_to_remove.append(key)
	
	for key in keys_to_remove:
		if telemetry_labels[key].name_label:
			telemetry_labels[key].name_label.queue_free()
		if telemetry_labels[key].value_label:
			telemetry_labels[key].value_label.queue_free()
		telemetry_labels.erase(key)
	
	# Update or create labels
	for key in data:
		var val = data[key]
		var val_text = ""
		
		if val is float:
			val_text = "%.3f" % val
		elif val is Vector3:
			val_text = "(%.2f, %.2f, %.2f)" % [val.x, val.y, val.z]
		else:
			val_text = str(val)
		
		if telemetry_labels.has(key):
			# Update existing label
			telemetry_labels[key].value_label.text = val_text
		else:
			# Create new labels
			var name_label = Label.new()
			name_label.text = str(key) + ":"
			name_label.add_theme_color_override("font_color", Color(0.8, 0.8, 0.8))
			telemetry_grid.add_child(name_label)
			
			var value_label = Label.new()
			value_label.text = val_text
			telemetry_grid.add_child(value_label)
			
			telemetry_labels[key] = {
				"name_label": name_label,
				"value_label": value_label
			}

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
