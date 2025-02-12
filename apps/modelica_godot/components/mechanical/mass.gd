class_name MassComponent
extends ModelicaComponent

func _init():
    add_connector("p", ModelicaConnector.Type.MECHANICAL)
    add_parameter("m", 1.0)  # Mass in kg
    add_variable("v", 0.0)   # Velocity
    add_variable("a", 0.0)   # Acceleration
    
    # F = ma
    add_equation("p.force = m * a")
    # v = dx/dt
    add_equation("v = der(p.position)")
    # a = dv/dt
    add_equation("a = der(v)") 