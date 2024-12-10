extends BaseFacility

# Input/output rates
var o2_input_rate: float = 1.0  # units/minute
var h2_input_rate: float = 2.0  # units/minute
var power_input_rate: float = 100.0  # kW
var h2o_output_rate: float = 1.0  # units/minute
var power_consumption: float = 100.0  # kW

# Current resource amounts
var o2_stored: float = 0.0
var h2_stored: float = 0.0
var power_available: float = 0.0

# Input nodes
var o2_source = null
var h2_source = null
var power_source = null
var h2o_storage = null

func _init():
	super._init()
	set_facility_properties("Factory", "Water production facility", "producer")
	efficiency = 0.95
	status = "Not Connected"  # Set initial status here too

func _ready() -> void:
	super._ready()
	update_status_display()
	

func connect_resource(node: Node, port: int) -> void:
	match port:
		0: o2_source = node
		1: h2_source = node
		2: power_source = node
		3: h2o_storage = node
	update_status_display()  # Update status when connection changes

func disconnect_resource(port: int) -> void:
	match port:
		0: o2_source = null
		1: h2_source = null
		2: power_source = null
		3: h2o_storage = null
	update_status_display()  # Update status when connection changes

func _process(delta: float) -> void:
	# First check if we have all required connections
	if not o2_source or not h2_source or not power_source or not h2o_storage:
		set_status("Not Connected")
		return
		
	# Check if we should change to Running status
	if status == "Not Connected":
		set_status("Running")
	
	# Only process if status is Running
	if status != "Running":
		return
		
	# Calculate how much we can produce based on inputs
	var minutes = delta * 60  # Convert seconds to minutes
	
	# Get resources from inputs
	var got_o2 = false
	var got_h2 = false
	var got_power = false
	
	if o2_source and "remove_resource" in o2_source:
		var o2_received = o2_source.remove_resource(o2_input_rate * minutes)
		if o2_received > 0:
			o2_stored += o2_received
			got_o2 = true
	
	if h2_source and "remove_resource" in h2_source:
		var h2_received = h2_source.remove_resource(h2_input_rate * minutes)
		if h2_received > 0:
			h2_stored += h2_received
			got_h2 = true
	
	if power_source and "power_output" in power_source:
		power_available = power_source.power_output * power_source.efficiency
		got_power = power_available >= power_input_rate
	
	# Update status based on resource availability
	if not got_o2:
		set_status("No O2 Input")
	elif not got_h2:
		set_status("No H2 Input")
	elif not got_power:
		set_status("Insufficient Power")
	else:
		set_status("Running")
	
	# Only produce if we have enough resources
	if status == "Running" and \
	   o2_stored >= o2_input_rate * minutes and \
	   h2_stored >= h2_input_rate * minutes and \
	   power_available >= power_input_rate:
		
		# Calculate production
		var h2o_produced = h2o_output_rate * efficiency * minutes
		
		# Consume resources
		o2_stored -= o2_input_rate * minutes
		h2_stored -= h2_input_rate * minutes
		
		# Output H2O
		if h2o_storage and "add_resource" in h2o_storage:
			h2o_storage.add_resource(h2o_produced)
	
	update_status_display()

func update_status_display() -> void:
	# Update display labels
	var status_label = $Parameters/Status
	if status_label:
		status_label.text = "Status: " + status
	
	var efficiency_label = $Parameters/Efficiency
	if efficiency_label:
		efficiency_label.text = "Efficiency: " + str(efficiency * 100) + "%"
	
	var power_label = $Parameters/PowerConsumption
	if power_label:
		power_label.text = "Power: " + str(power_consumption) + " kW"
	
	var o2_label = $Parameters/O2Level
	if o2_label:
		o2_label.text = "O2: %.2f units" % o2_stored
	
	var h2_label = $Parameters/H2Level
	if h2_label:
		h2_label.text = "H2: %.2f units" % h2_stored
  
