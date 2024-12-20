extends UIBaseFacility

func _ready():
	# Set up input and output ports
	set_slot(0, true, 0, Color.BLUE, true, 0, Color.BLUE)  # Resource flow
	set_slot(1, true, 0, Color.YELLOW, false, 0, Color.YELLOW)  # Power input 
