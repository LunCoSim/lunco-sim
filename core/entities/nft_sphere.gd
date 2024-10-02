extends Node3D

@onready var mesh: MeshInstance3D = $MeshInstance3D
@onready var label: Label3D = $Label3D

var nft_data: Dictionary = {}

func _ready():
	if not label:
		print("Warning: Label3D node not found in NFT Sphere scene")

func set_nft_data(data: Dictionary):
	nft_data = data
	update_appearance()

func update_appearance():
	if nft_data.has("name") and label:
		label.text = nft_data["name"]
	
	if nft_data.has("color") and mesh:
		var material = StandardMaterial3D.new()
		material.albedo_color = Color(nft_data["color"])
		mesh.material_override = material
