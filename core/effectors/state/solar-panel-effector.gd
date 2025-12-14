class_name LCSolarPanelEffector
extends LCStateEffector

## Solar panel state effector for power generation.
##
## Generates power based on sun angle and panel area.
## Supports deployable and articulated panels.

@export_group("Solar Panel Properties")
@export var panel_area: float = 1.0  ## Panel area in m²
@export var panel_efficiency: float = 0.3  ## Solar cell efficiency (0.0 to 1.0)
@export var max_power_output: float = 300.0  ## Maximum power output in Watts
@export var panel_normal: Vector3 = Vector3(0, 0, 1)  ## Local panel normal direction

@export_group("Sun Tracking")
@export var can_articulate: bool = false  ## Can panel rotate to track sun?
@export var articulation_rate: float = 1.0  ## Rotation rate in deg/s
@export var articulation_axis: Vector3 = Vector3(0, 1, 0)  ## Local rotation axis

@export_group("Deployment")
@export var is_deployable: bool = false  ## Is panel deployable?
@export var is_deployed: bool = true  ## Current deployment state
@export var deployment_time: float = 10.0  ## Time to fully deploy in seconds

@export_group("Environment")
@export var sun_direction: Vector3 = Vector3(1, 0, 0)  ## Global sun direction (updated by environment)
@export var solar_flux: float = 1361.0  ## Solar flux in W/m² (1361 = Earth orbit)

# Internal state
var current_power_output: float = 0.0
var deployment_fraction: float = 1.0  ## 0.0 = stowed, 1.0 = deployed
var panel_angle: float = 0.0  ## Current articulation angle in degrees
var target_angle: float = 0.0  ## Target articulation angle
var total_energy_generated: float = 0.0  ## Total energy in Watt-hours

# Solver Integration
# Inherited from LCStateEffector:
# var solver_graph: LCSolverGraph
# var solver_node: LCSolverNode  ## Electrical node representing panel output

# Constants
const AU: float = 1.496e11  ## Astronomical unit in meters

func _ready():
	super._ready()
	panel_normal = panel_normal.normalized()
	articulation_axis = articulation_axis.normalized()
	mass = 5.0 + panel_area * 2.0  # Rough mass estimate
	
	if is_deployable and not is_deployed:
		deployment_fraction = 0.0
	
	_initialize_telemetry()

## Set the solver graph (called by spacecraft during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and not solver_node:
		# Create electrical source node
		solver_node = solver_graph.add_node(0.0, false, "Electrical")
		solver_node.resource_type = "electrical_power"
		solver_node.display_name = name  # Use effector's name
		solver_node.effector_ref = weakref(self)
		
		# Solar panel acts as a current source
		# We'll update flow_source based on power output
		
		print("SolarPanel: Created solver node as current source")

func _physics_process(delta):
	_update_deployment(delta)
	_update_articulation(delta)
	_update_power_generation(delta)

func _process(delta):
	_update_telemetry()

## Deploys the solar panel.
func deploy():
	if is_deployable:
		is_deployed = true

## Stows the solar panel.
func stow():
	if is_deployable:
		is_deployed = false

## Sets the target articulation angle in degrees.
func set_articulation_angle(angle_deg: float):
	if can_articulate:
		target_angle = angle_deg

## Enables sun tracking (auto-articulation).
func enable_sun_tracking():
	if can_articulate:
		# Calculate optimal angle to face sun
		var sun_dir_local = global_transform.basis.inverse() * sun_direction.normalized()
		var current_normal = Basis(articulation_axis, deg_to_rad(panel_angle)) * panel_normal
		
		# Calculate angle between current normal and sun
		var dot_product = current_normal.dot(sun_dir_local)
		var angle_to_sun = rad_to_deg(acos(clamp(dot_product, -1.0, 1.0)))
		
		# Simple proportional control
		target_angle = panel_angle + angle_to_sun * 0.1

## Updates deployment state.
func _update_deployment(delta: float):
	if is_deployable:
		if is_deployed and deployment_fraction < 1.0:
			deployment_fraction += delta / deployment_time
			deployment_fraction = min(1.0, deployment_fraction)
		elif not is_deployed and deployment_fraction > 0.0:
			deployment_fraction -= delta / deployment_time
			deployment_fraction = max(0.0, deployment_fraction)

## Updates panel articulation.
func _update_articulation(delta: float):
	if can_articulate:
		var angle_delta = articulation_rate * delta
		if panel_angle < target_angle:
			panel_angle = min(panel_angle + angle_delta, target_angle)
		elif panel_angle > target_angle:
			panel_angle = max(panel_angle - angle_delta, target_angle)

## Updates power generation based on sun angle.
func _update_power_generation(delta: float):
	if deployment_fraction <= 0.0:
		current_power_output = 0.0
		power_production = 0.0
		if solver_node:
			solver_node.flow_source = 0.0
		return
	
	# Calculate effective panel normal with articulation
	var effective_normal = panel_normal
	if can_articulate:
		var rotation = Basis(articulation_axis, deg_to_rad(panel_angle))
		effective_normal = rotation * panel_normal
	
	# Convert to global frame
	var global_normal = global_transform.basis * effective_normal
	
	# Calculate sun angle (cosine of angle between panel normal and sun direction)
	var sun_dir_normalized = sun_direction.normalized()
	var cos_angle = global_normal.dot(sun_dir_normalized)
	cos_angle = max(0.0, cos_angle)  # No power when sun is behind panel
	
	# Calculate power output
	var effective_area = panel_area * deployment_fraction
	var incident_power = solar_flux * effective_area * cos_angle
	current_power_output = incident_power * panel_efficiency
	current_power_output = min(current_power_output, max_power_output)
	
	power_production = current_power_output
	
	# Track total energy
	total_energy_generated += current_power_output * delta / 3600.0  # Convert to Wh
	
	# Update solver node (inject current)
	if solver_node:
		# Assume nominal bus voltage for current calculation
		# I = P / V
		var bus_voltage = 28.0  # Typical spacecraft bus voltage
		if solver_node.potential > 1.0:
			bus_voltage = solver_node.potential
		
		var current_amps = current_power_output / bus_voltage
		solver_node.flow_source = current_amps

## Returns the current sun angle in degrees.
func get_sun_angle() -> float:
	var effective_normal = panel_normal
	if can_articulate:
		var rotation = Basis(articulation_axis, deg_to_rad(panel_angle))
		effective_normal = rotation * panel_normal
	
	var global_normal = global_transform.basis * effective_normal
	var sun_dir_normalized = sun_direction.normalized()
	var cos_angle = global_normal.dot(sun_dir_normalized)
	return rad_to_deg(acos(clamp(cos_angle, -1.0, 1.0)))

## Returns true if panel is generating power.
func is_generating_power() -> bool:
	return current_power_output > 0.1

## Updates sun direction (called by environment system).
func update_sun_direction(new_sun_dir: Vector3):
	sun_direction = new_sun_dir

## Updates solar flux (called by environment system).
func update_solar_flux(new_flux: float):
	solar_flux = new_flux

func _initialize_telemetry():
	Telemetry = {
		"power_output": current_power_output,
		"deployment_fraction": deployment_fraction,
		"is_deployed": is_deployed,
		"panel_angle": panel_angle,
		"sun_angle": get_sun_angle(),
		"total_energy": total_energy_generated,
	}

func _update_telemetry():
	Telemetry["power_output"] = current_power_output
	Telemetry["deployment_fraction"] = deployment_fraction
	Telemetry["is_deployed"] = is_deployed
	Telemetry["panel_angle"] = panel_angle
	Telemetry["sun_angle"] = get_sun_angle()
	Telemetry["total_energy"] = total_energy_generated

# Command interface
func cmd_deploy(args: Array):
	deploy()

func cmd_stow(args: Array):
	stow()

func cmd_set_angle(args: Array):
	if args.size() >= 1:
		set_articulation_angle(args[0])

func cmd_track_sun(args: Array):
	enable_sun_tracking()
