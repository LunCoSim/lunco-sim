extends LCControllerUI

# target is inherited from LCControllerUI
@onready var target_typed: Node3D = target


func _on_HideControls_timeout():
	$PanelContainer.visible = false
