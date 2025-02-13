@tool
extends Control

var spring_rest_length := 100.0
var spring_segments := 20
var mass_radius := 20.0

# Visual properties
var fixed_point_pos := Vector2(100, 200)
var mass_color := Color(0.2, 0.6, 1.0)
var spring_color := Color(0.8, 0.3, 0.3)
var fixed_color := Color(0.3, 0.3, 0.3)

func _draw() -> void:
	# Draw fixed point
	draw_circle(fixed_point_pos, 10.0, fixed_color)
	
	# Get mass position from the last drawn position
	var mass_pos := get_mass_position()
	
	# Draw spring
	draw_spring(fixed_point_pos, mass_pos)
	
	# Draw mass
	draw_circle(mass_pos, mass_radius, mass_color)

func draw_spring(start: Vector2, end: Vector2) -> void:
	var direction := end - start
	var length := direction.length()
	var segment_length := length / spring_segments
	var perpendicular := direction.orthogonal().normalized() * 20.0
	
	var points := PackedVector2Array()
	points.append(start)
	
	for i in range(spring_segments):
		var t := float(i) / spring_segments
		var base_point := start + direction * t
		
		if i % 2 == 1:
			points.append(base_point + perpendicular)
		else:
			points.append(base_point - perpendicular)
	
	points.append(end)
	
	# Draw the spring
	draw_polyline(points, spring_color, 2.0)

func get_mass_position() -> Vector2:
	# This will be updated by the simulation
	return fixed_point_pos + Vector2(spring_rest_length, 0)

func update_spring_mass_damper(position: float) -> void:
	# Update the mass position based on simulation
	spring_rest_length = 100.0 + position * 50.0  # Scale the position for visualization
	queue_redraw()  # Request a redraw of the visualization 