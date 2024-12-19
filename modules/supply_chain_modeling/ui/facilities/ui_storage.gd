extends UIBaseFacility

var storage: StorageFacility

func _init():
	super._init()
	# set_facility_properties("Storage", "Generic storage facility", "storage")
	storage = StorageFacility.new()

func _ready():
	super._ready()
	update_status_display()

func update_status_display() -> void:
	var capacity_label = $VBoxContainer/Label
	if capacity_label:
		capacity_label.text = "Capacity: " + str(storage.capacity)
	
	var progress_bar = $VBoxContainer/ProgressBar
	if progress_bar:
		progress_bar.max_value = storage.capacity
		progress_bar.value = storage.current_amount

