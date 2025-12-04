extends GraphNode

var solver_node: LCSolverNode

@onready var label_id = $VBoxContainer/LabelID
@onready var label_domain = $VBoxContainer/LabelDomain
@onready var label_potential = $VBoxContainer/LabelPotential
@onready var label_flow = $VBoxContainer/LabelFlow
@onready var label_resource = $VBoxContainer/LabelResource

# Configuration
const MIN_SIZE_X = 160.0
const MAX_SIZE_X = 240.0

func _ready():
	if solver_node:
		title = "Node " + str(solver_node.id)
		label_id.text = "ID: " + str(solver_node.id)
		label_domain.text = str(solver_node.domain)
		
		# Set initial size based on capacity if it's a storage node
		if solver_node.is_storage and solver_node.capacitance > 0:
			# Logarithmic scaling for size
			var size_factor = clamp(log(solver_node.capacitance) / 10.0, 0.0, 1.0)
			custom_minimum_size.x = lerp(MIN_SIZE_X, MAX_SIZE_X, size_factor)
			size.x = custom_minimum_size.x

func _process(_delta):
	if not solver_node:
		return
		
	label_potential.text = "P: %.2f" % solver_node.potential
	label_flow.text = "Acc: %.2f" % solver_node.flow_accumulation
	
	if solver_node.resource_type != "":
		label_resource.text = str(solver_node.resource_type)
	else:
		label_resource.text = "-"
		
	_update_visuals()

func _update_visuals():
	# Color coding based on domain and state
	var target_color = Color.WHITE
	
	match solver_node.domain:
		"Fluid": target_color = Color(0.2, 0.6, 1.0) # Blue
		"Electrical": target_color = Color(1.0, 0.8, 0.2) # Yellow
		"Thermal": target_color = Color(1.0, 0.4, 0.2) # Orange
	
	# If storage, blend with fill level color
	if solver_node.is_storage and solver_node.capacitance > 0:
		var fill_level = clamp(solver_node.flow_accumulation / solver_node.capacitance, 0.0, 1.0)
		# Red (empty) -> Yellow (half) -> Green (full)
		var fill_color = Color.RED.lerp(Color.YELLOW, fill_level * 2.0) if fill_level < 0.5 else Color.YELLOW.lerp(Color.GREEN, (fill_level - 0.5) * 2.0)
		
		# Blend domain color with fill color (70% fill color, 30% domain color)
		self_modulate = target_color.lerp(fill_color, 0.7)
		
		# Add glow if full or nearly full
		if fill_level > 0.95:
			self_modulate = self_modulate * 1.2
	else:
		self_modulate = target_color
		
		# For non-storage nodes, visualize potential intensity
		# e.g. High pressure/voltage = brighter
		# This is arbitrary scaling, assuming 100 is "high"
		var potential_factor = clamp(abs(solver_node.potential) / 100.0, 0.0, 0.5)
		self_modulate = self_modulate.lightened(potential_factor)

