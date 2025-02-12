class_name SpringGraphNode
extends ComponentGraphNode

var force_label: Label
var extension_label: Label

func _init(component_: SpringComponent):
	super(component_)
	
	# Add visualization elements
	force_label = Label.new()
	force_label.text = "Force: 0.0 N"
	add_child(force_label)
	
	extension_label = Label.new()
	extension_label.text = "Extension: 0.0 m"
	add_child(extension_label)

func update_visualization() -> void:
	var spring_component = component as SpringComponent
	var p1 = spring_component.get_connector("p1")
	var p2 = spring_component.get_connector("p2")
	
	var force = p1.get_value("force")
	var extension = p2.get_value("position") - p1.get_value("position")
	
	force_label.text = "Force: %.2f N" % force
	extension_label.text = "Extension: %.2f m" % extension 
