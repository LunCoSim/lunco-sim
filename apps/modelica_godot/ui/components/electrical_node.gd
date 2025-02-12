class_name ElectricalGraphNode
extends ComponentGraphNode

var voltage_label: Label
var current_label: Label

func _init(component_: ModelicaComponent):
	super(component_)
	
	voltage_label = Label.new()
	voltage_label.text = "Voltage: 0.0 V"
	add_child(voltage_label)
	
	current_label = Label.new()
	current_label.text = "Current: 0.0 A"
	add_child(current_label)

func update_visualization() -> void:
	var p = component.get_connector("p")
	var n = component.get_connector("n")
	
	var voltage = p.get_value("voltage") - n.get_value("voltage")
	var current = p.get_value("current")
	
	voltage_label.text = "Voltage: %.2f V" % voltage
	current_label.text = "Current: %.2f A" % current 
