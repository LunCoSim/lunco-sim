extends Node3D

var missions = preload("res://addons/lunco-content/datasets/missions.csv")
var landings = []
#print(missions.records)


# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.
	
#	for i in missions.records:
#		print(i["Mission type"]=="landing")
	
	landings = missions.records.filter(func(i): return i["Mission type"]=="landing")
	landings = landings.filter(func(i): return i["Landing location"]!="tbd")
	
	var count := 0
	
	for i in landings:
		count += 1
		var sphere : Node3D = preload("res://apps/future_lunar_missions/sphere.tscn").instantiate()
		
		var r = 3
		var lat = float(i["latitude"])
		var lon = float(i["longitude"])
		
		sphere.translate(spherical_to_cartesian(r, lat, lon))
		
		var material = StandardMaterial3D.new()
		
		material.albedo_color = Color(abs(sin(count)), abs(cos(count)), abs(cos(0.3*count)))
		
		sphere.mesh.surface_set_material(0, material)
		
		print(material.albedo_color )
		$Moon.add_child(sphere)
		
	print(landings)

static func spherical_to_cartesian(r, phi_degrees, theta_degrees) -> Vector3:
	var location := Vector3.ZERO
	
	var phi = deg_to_rad(phi_degrees) - 3.14/2
	var theta = deg_to_rad(theta_degrees)
	
	location.z = r*sin(phi)*cos(theta)
	location.x = r*sin(phi)*sin(theta)
	location.y = r*cos(phi)
	
	return location

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass
