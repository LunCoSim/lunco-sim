class_name GroundComponent
extends ModelicaComponent

func _init():
    add_connector("p", ModelicaConnector.Type.MECHANICAL)
    
    # Ground position is fixed at 0
    add_equation("p.position = 0")
    # Ground can provide any force needed
    add_variable("reaction_force", 0.0)
    add_equation("p.force = reaction_force") 