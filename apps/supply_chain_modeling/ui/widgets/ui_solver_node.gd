extends GraphNode

var solver_node: LCSolverNode

@onready var label_id = $VBoxContainer/LabelID
@onready var label_domain = $VBoxContainer/LabelDomain
@onready var label_potential = $VBoxContainer/LabelPotential
@onready var label_flow = $VBoxContainer/LabelFlow
@onready var label_resource = $VBoxContainer/LabelResource

func _ready():
	if solver_node:
		title = "Solver Node " + str(solver_node.id)
		label_id.text = "ID: " + str(solver_node.id)
		label_domain.text = "Domain: " + str(solver_node.domain)
		
		# Set color based on domain
		match solver_node.domain:
			"Fluid": self_modulate = Color(0.2, 0.6, 1.0) # Blue
			"Electrical": self_modulate = Color(1.0, 0.8, 0.2) # Yellow
			"Thermal": self_modulate = Color(1.0, 0.4, 0.2) # Orange
			_: self_modulate = Color.WHITE

func _process(_delta):
	if solver_node:
		label_potential.text = "P: %.2f" % solver_node.potential
		label_flow.text = "Acc: %.2f" % solver_node.flow_accumulation
		
		if solver_node.resource_type != "":
			label_resource.text = "Res: " + str(solver_node.resource_type)
		else:
			label_resource.text = ""
			
		# Visual feedback for potential (e.g., brightness)
		# var intensity = clamp(solver_node.potential / 100.0, 0.2, 1.0)
		# modulate.a = intensity
