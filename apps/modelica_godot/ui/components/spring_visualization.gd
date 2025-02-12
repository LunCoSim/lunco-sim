extends Node2D

const SPRING_COLOR = Color(0.2, 0.6, 1.0)
const SPRING_WIDTH = 2.0
const COIL_COUNT = 10
const ENDPOINT_RADIUS = 5.0

var spring_constant: float = 100.0
var rest_length: float = 1.0
var current_length: float = 1.0
var force: float = 0.0

# Visual properties
var start_pos: Vector2
var end_pos: Vector2

func _ready():
	start_pos = Vector2.ZERO
	end_pos = Vector2(current_length * 100, 0)  # Scale meters to pixels (100 pixels = 1 meter)

func set_spring_constant(k: float):
	spring_constant = k
	queue_redraw()

func set_rest_length(length: float):
	rest_length = length
	queue_redraw()

func set_current_length(length: float):
	current_length = length
	end_pos.x = length * 100  # Scale meters to pixels
	queue_redraw()

func set_force(f: float):
	force = f
	queue_redraw()

func get_spring_constant() -> float:
	return spring_constant

func get_rest_length() -> float:
	return rest_length

func _draw():
	# Draw endpoints
	draw_circle(start_pos, ENDPOINT_RADIUS, Color.WHITE)
	draw_circle(end_pos, ENDPOINT_RADIUS, Color.WHITE)
	
	# Draw spring
	var direction = (end_pos - start_pos).normalized()
	var length = end_pos.distance_to(start_pos)
	var segment_length = length / (COIL_COUNT * 4)
	
	var current_pos = start_pos
	var points = []
	var coil_height = 20.0  # Height of spring coils
	
	# Draw straight line at start
	points.append(current_pos)
	current_pos += direction * segment_length
	points.append(current_pos)
	
	# Draw coils
	for i in range(COIL_COUNT):
		var perpendicular = Vector2(-direction.y, direction.x)
		
		# Top of coil
		current_pos += direction * segment_length
		points.append(current_pos + perpendicular * coil_height)
		
		# Bottom of coil
		current_pos += direction * segment_length
		points.append(current_pos - perpendicular * coil_height)
	
	# Draw straight line at end
	current_pos += direction * segment_length
	points.append(current_pos)
	points.append(end_pos)
	
	# Draw the spring
	for i in range(points.size() - 1):
		draw_line(points[i], points[i + 1], SPRING_COLOR, SPRING_WIDTH)
	
	# Draw force indicator
	var force_scale = 0.01  # Scale factor for force visualization
	var force_arrow = Vector2(force * force_scale, 0)
	var arrow_pos = end_pos + Vector2(20, 0)
	draw_line(arrow_pos, arrow_pos + force_arrow, Color.RED, 2.0)
	
	# Draw labels
	var font = ThemeDB.fallback_font
	var font_size = 16
	draw_string(font, end_pos + Vector2(0, 30), "Length: %.2f m" % current_length)
	draw_string(font, end_pos + Vector2(0, 50), "Force: %.2f N" % force) 