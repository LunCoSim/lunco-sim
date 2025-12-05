class_name NodeDetailsPanel
extends PanelContainer

@onready var label_title = $MarginContainer/VBoxContainer/Header/LabelTitle
@onready var label_id = $MarginContainer/VBoxContainer/GridContainer/LabelIDValue
@onready var label_domain = $MarginContainer/VBoxContainer/GridContainer/LabelDomainValue
@onready var label_potential = $MarginContainer/VBoxContainer/GridContainer/LabelPotentialValue
@onready var label_flow = $MarginContainer/VBoxContainer/GridContainer/LabelFlowValue
@onready var label_resource = $MarginContainer/VBoxContainer/GridContainer/LabelResourceValue
@onready var label_capacitance = $MarginContainer/VBoxContainer/GridContainer/LabelCapacitanceValue
@onready var label_capacitance_label = $MarginContainer/VBoxContainer/GridContainer/LabelCapacitance

var current_node: LCSolverNode

func _ready():
	hide()

func _process(_delta):
	if visible and current_node:
		_update_values()

func display_node(node: LCSolverNode):
	current_node = node
	show()
	_update_static_info()
	_update_values()

func _update_static_info():
	if not current_node: return
	label_id.text = str(current_node.id)
	label_domain.text = str(current_node.domain)
	
	if current_node.is_storage:
		label_capacitance.text = "%.2f" % current_node.capacitance
		label_capacitance.show()
		label_capacitance_label.show()
	else:
		label_capacitance.hide()
		label_capacitance_label.hide()

func _update_values():
	if not current_node: return
	label_potential.text = "%.4f" % current_node.potential
	label_flow.text = "%.4f" % current_node.flow_accumulation
	label_resource.text = current_node.resource_type if current_node.resource_type else "None"

func _on_close_button_pressed():
	hide()
	current_node = null
