extends PanelContainer

@onready var tabs = $VBoxContainer/TabContainer
@onready var label = $VBoxContainer/InstructionLabel
@onready var sym_none = $VBoxContainer/SymmetryControls/BtnNoSym
@onready var sym_x = $VBoxContainer/SymmetryControls/BtnXSym
@onready var sym_z = $VBoxContainer/SymmetryControls/BtnZSym
@onready var sym_quad = $VBoxContainer/SymmetryControls/BtnQuadSym

var selected_button: Button = null

func _ready():
	var bm = get_node_or_null("/root/BuilderManager")
	if not bm: return
	
	# Connect signals
	if not bm.part_deselected.is_connected(_on_part_deselected):
		bm.part_deselected.connect(_on_part_deselected)
		
	# Setup Symmetry Buttons
	var group = ButtonGroup.new()
	sym_none.button_group = group
	sym_x.button_group = group
	sym_z.button_group = group
	sym_quad.button_group = group
	
	sym_none.pressed.connect(func(): _set_symmetry(bm.SymmetryMode.NONE))
	sym_x.pressed.connect(func(): _set_symmetry(bm.SymmetryMode.X))
	sym_z.pressed.connect(func(): _set_symmetry(bm.SymmetryMode.Z))
	sym_quad.pressed.connect(func(): _set_symmetry(bm.SymmetryMode.QUAD))

	# Populate Categories
	for category in bm.categories:
		var tab_name = category
		var parts = bm.categories[category]
		var container = _get_tab_container(tab_name)
		
		if container:
			for part_id in parts:
				var btn = Button.new()
				btn.text = part_id.capitalize().replace("Effector", "").replace("_", " ")
				btn.alignment = HORIZONTAL_ALIGNMENT_LEFT
				btn.pressed.connect(_on_part_selected.bind(part_id, btn))
				container.add_child(btn)

func _get_tab_container(name: String) -> VBoxContainer:
	var node = tabs.get_node_or_null(name + "/VBox")
	return node

func _set_symmetry(mode):
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		bm.symmetry_mode = mode

func _on_part_selected(part_id: String, btn: Button):
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		bm.select_part(part_id)
		
		# Visual feedback
		if selected_button:
			selected_button.modulate = Color(1, 1, 1)
		selected_button = btn
		btn.modulate = Color(0.5, 1.0, 0.5)
		
		label.text = "Placing: " + btn.text

func _on_part_deselected():
	if selected_button:
		selected_button.modulate = Color(1, 1, 1)
		selected_button = null
	label.text = "Select a part..."

