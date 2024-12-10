extends BaseFacility

var capacity: float = 100.0  # Maximum storage capacity
var current_amount: float = 0.0  # Current amount stored
var resource_type: String = ""  # Type of resource being stored

func _init():
    super._init()
    set_facility_properties("Storage", "Generic storage facility", "storage")

func process_resources(delta: float) -> void:
    if status != "Running":
        return
    
    # Implementation will depend on how resource flow is handled

func update_status_display() -> void:
    var capacity_label = $VBoxContainer/Label
    if capacity_label:
        capacity_label.text = "Capacity: " + str(capacity)
    
    var progress_bar = $VBoxContainer/ProgressBar
    if progress_bar:
        progress_bar.max_value = capacity
        progress_bar.value = current_amount

func add_resource(amount: float) -> float:
    var space_available = capacity - current_amount
    var amount_to_add = min(amount, space_available)
    current_amount += amount_to_add
    update_status_display()
    return amount_to_add

func remove_resource(amount: float) -> float:
    var amount_to_remove = min(amount, current_amount)
    current_amount -= amount_to_remove
    update_status_display()
    return amount_to_remove 