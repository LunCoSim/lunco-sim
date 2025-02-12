class_name ResistorComponent
extends ModelicaComponent

func _init():
	add_connector("p", ModelicaConnector.Type.ELECTRICAL)
	add_connector("n", ModelicaConnector.Type.ELECTRICAL)
	add_parameter("R", 100.0)  # Resistance in ohms
	
	# Ohm's law: V = IR
	add_equation("p.voltage - n.voltage = R * p.current")
	# Current through both terminals is equal and opposite
	add_equation("p.current + n.current = 0") 
