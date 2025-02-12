class_name SpringComponent
extends ModelicaComponent

func _init():
    add_connector("p1", ModelicaConnector.Type.MECHANICAL)
    add_connector("p2", ModelicaConnector.Type.MECHANICAL)
    add_parameter("k", 100.0)  # Spring constant N/m
    add_parameter("d", 0.1)    # Damping coefficient N⋅s/m
    
    # Hooke's law: F = -k * x
    add_equation("p1.force = k * (p2.position - p1.position)")
    # Add damping: F = -d * v
    add_equation("p1.force = p1.force + d * der(p2.position - p1.position)")
    # Newton's third law
    add_equation("p1.force + p2.force = 0") 