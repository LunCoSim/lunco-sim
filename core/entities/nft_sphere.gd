extends Node3D

var mesh: MeshInstance3D
var label: Label3D

var nft_data: Dictionary = {}

func _ready():
	print("NFT Sphere instantiated")
	mesh = get_node("MeshInstance3D")
	label = get_node("Label3D")
	
	if not mesh:
		print("Error: MeshInstance3D node not found in NFT Sphere scene")
	if not label:
		print("Error: Label3D node not found in NFT Sphere scene")
	
	# If nft_data was set before _ready, update appearance now
	if not nft_data.is_empty():
		update_appearance()

func set_nft_data(data: Dictionary):
	print("set_nft_data called with: ", data)  # Debug print
	nft_data = data
	if is_inside_tree():
		update_appearance()

func update_appearance():
	print("update_appearance called with nft_data: ", nft_data)  # Debug print
	
	if nft_data.has("name") and label:
		label.text = nft_data["name"]
		print("Updated label text to: ", label.text)
	else:
		print("Failed to update label. Label exists: ", label != null, ", nft_data has name: ", nft_data.has("name"))
	
	if mesh:
		var material = StandardMaterial3D.new()
		var color = Color.WHITE  # Default color is white
		if nft_data.has("color"):
			color = Color.from_string(nft_data["color"], Color.WHITE)
		material.albedo_color = color
		mesh.material_override = material
		print("Updated mesh color to: ", color)
	else:
		print("Failed to update mesh color. Mesh exists: ", mesh != null)

	# Force update of the node
	if label:
		label.set_text(label.text)
	if mesh:
		mesh.set_surface_override_material(0, mesh.get_surface_override_material(0))
