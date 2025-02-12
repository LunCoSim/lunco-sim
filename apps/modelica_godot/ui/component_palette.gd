@tool
extends PanelContainer

signal component_selected(type: String)

func _ready():
	# Connect button signals
	var mass_btn = $VBoxContainer/ScrollContainer/ComponentList/MassBtn
	var spring_btn = $VBoxContainer/ScrollContainer/ComponentList/SpringBtn
	var fixed_btn = $VBoxContainer/ScrollContainer/ComponentList/FixedBtn
	
	mass_btn.pressed.connect(_on_mass_selected)
	spring_btn.pressed.connect(_on_spring_selected)
	fixed_btn.pressed.connect(_on_fixed_selected)

func _on_mass_selected():
	emit_signal("component_selected", "Mass")

func _on_spring_selected():
	emit_signal("component_selected", "Spring")

func _on_fixed_selected():
	emit_signal("component_selected", "Fixed") 