@tool
extends Control

signal simulation_started
signal simulation_paused
signal simulation_reset
signal simulation_step(delta: float)

var is_simulating := false
var simulation_time := 0.0
var plot_data := {
	"time": [],
	"position": [],
	"velocity": [],
	"acceleration": []
}

@onready var play_button := $Controls/PlayButton
@onready var reset_button := $Controls/ResetButton
@onready var time_label := $Controls/TimeLabel
@onready var plot_container := $PlotContainer
@onready var visualization := $Visualization

func _ready() -> void:
	play_button.pressed.connect(_on_play_pressed)
	reset_button.pressed.connect(_on_reset_pressed)
	
	# Initialize plots
	setup_plots()

func _process(delta: float) -> void:
	if is_simulating:
		simulation_time += delta
		time_label.text = "Time: %.2f s" % simulation_time
		emit_signal("simulation_step", delta)

func _on_play_pressed() -> void:
	is_simulating = !is_simulating
	play_button.text = "Pause" if is_simulating else "Play"
	
	if is_simulating:
		emit_signal("simulation_started")
	else:
		emit_signal("simulation_paused")

func _on_reset_pressed() -> void:
	is_simulating = false
	simulation_time = 0.0
	play_button.text = "Play"
	time_label.text = "Time: 0.00 s"
	
	# Clear plot data
	plot_data.time.clear()
	plot_data.position.clear()
	plot_data.velocity.clear()
	plot_data.acceleration.clear()
	
	emit_signal("simulation_reset")
	update_plots()

func setup_plots() -> void:
	# Create position plot
	var position_plot := Line2D.new()
	position_plot.default_color = Color.GREEN
	position_plot.width = 2.0
	plot_container.add_child(position_plot)
	
	# Create velocity plot
	var velocity_plot := Line2D.new()
	velocity_plot.default_color = Color.BLUE
	velocity_plot.width = 2.0
	plot_container.add_child(velocity_plot)
	
	# Create acceleration plot
	var acceleration_plot := Line2D.new()
	acceleration_plot.default_color = Color.RED
	acceleration_plot.width = 2.0
	plot_container.add_child(acceleration_plot)

func update_state(position: float, velocity: float, acceleration: float) -> void:
	# Update plot data
	plot_data.time.append(simulation_time)
	plot_data.position.append(position)
	plot_data.velocity.append(velocity)
	plot_data.acceleration.append(acceleration)
	
	# Update visualization
	visualization.update_spring_mass_damper(position)
	
	# Update plots
	update_plots()

func update_plots() -> void:
	var max_points := 1000  # Maximum number of points to show
	
	# If we have too many points, remove old ones
	if plot_data.time.size() > max_points:
		var excess: int = plot_data.time.size() - max_points
		plot_data.time = plot_data.time.slice(excess)
		plot_data.position = plot_data.position.slice(excess)
		plot_data.velocity = plot_data.velocity.slice(excess)
		plot_data.acceleration = plot_data.acceleration.slice(excess)
	
	# Update position plot
	var position_points := PackedVector2Array()
	for i in plot_data.time.size():
		position_points.append(Vector2(
			plot_container.size.x * (plot_data.time[i] / 10.0),  # X coordinate (time)
			plot_container.size.y * 0.5 - plot_data.position[i] * 50.0  # Y coordinate (position)
		))
	$PlotContainer/PositionPlot.points = position_points
	
	# Update velocity plot
	var velocity_points := PackedVector2Array()
	for i in plot_data.time.size():
		velocity_points.append(Vector2(
			plot_container.size.x * (plot_data.time[i] / 10.0),
			plot_container.size.y * 0.5 - plot_data.velocity[i] * 20.0
		))
	$PlotContainer/VelocityPlot.points = velocity_points
	
	# Update acceleration plot
	var acceleration_points := PackedVector2Array()
	for i in plot_data.time.size():
		acceleration_points.append(Vector2(
			plot_container.size.x * (plot_data.time[i] / 10.0),
			plot_container.size.y * 0.5 - plot_data.acceleration[i] * 10.0
		))
	$PlotContainer/AccelerationPlot.points = acceleration_points 