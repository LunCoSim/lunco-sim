class_name MassGraphNode
extends ComponentGraphNode

var position_label: Label
var velocity_label: Label

func _init(component_: MassComponent):
    super(component_)
    
    # Add visualization elements
    position_label = Label.new()
    position_label.text = "Position: 0.0"
    add_child(position_label)
    
    velocity_label = Label.new()
    velocity_label.text = "Velocity: 0.0"
    add_child(velocity_label)

func update_visualization() -> void:
    var mass_component = component as MassComponent
    position_label.text = "Position: %.2f" % mass_component.get_connector("p").get_value("position")
    velocity_label.text = "Velocity: %.2f" % mass_component.get_variable("v") 