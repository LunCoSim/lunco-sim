class_name VoltageSourceComponent
extends ModelicaComponent

func _init():
	add_connector("p", ModelicaConnector.Type.ELECTRICAL)
	add_connector("n", ModelicaConnector.Type.ELECTRICAL)
	add_parameter("V", 12.0)  # Voltage in volts
	
	# Set voltage difference between terminals
	add_equation("p.voltage - n.voltage = V")
	# Current through both terminals is equal and opposite
	add_equation("p.current + n.current = 0") 
