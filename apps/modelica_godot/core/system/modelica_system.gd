class_name ModelicaSystem
extends RefCounted

var components: Dictionary = {}  # name -> ModelicaComponent
var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step
var is_initialized: bool = false

signal state_changed(component_name: String, variable_name: String, value: float)
signal parameter_changed(component_name: String, param_name: String, value: Variant)
signal event_triggered(component_name: String, event_name: String, data: Dictionary)

func _init() -> void:
    equation_system = EquationSystem.new()

func add_component(component: ModelicaComponent) -> void:
    var name = component.get_declaration(component.declarations.keys()[0]).name
    components[name] = component
    
    # Connect signals
    component.state_changed.connect(_on_component_state_changed.bind(name))
    component.parameter_changed.connect(_on_component_parameter_changed.bind(name))
    component.event_triggered.connect(_on_component_event_triggered.bind(name))

func get_component(name: String) -> ModelicaComponent:
    return components.get(name)

func connect_components(from_component: String, from_port: String, 
                      to_component: String, to_port: String) -> bool:
    var from_comp = get_component(from_component)
    var to_comp = get_component(to_component)
    
    if not from_comp or not to_comp:
        push_error("Component not found")
        return false
    
    var from_connector = from_comp.get_connector(from_port)
    var to_connector = to_comp.get_connector(to_port)
    
    if not from_connector or not to_connector:
        push_error("Connector not found")
        return false
    
    if from_connector.type != to_connector.type:
        push_error("Cannot connect different connector types")
        return false
    
    # Add connection equations
    for var_name in from_connector.variables.keys():
        if from_connector.get_variable(var_name).is_flow_variable():
            # Through variables sum to zero
            equation_system.add_equation(
                "%s.%s.%s + %s.%s.%s = 0" % [
                    from_component, from_port, var_name,
                    to_component, to_port, var_name
                ]
            )
        else:
            # Across variables are equal
            equation_system.add_equation(
                "%s.%s.%s = %s.%s.%s" % [
                    from_component, from_port, var_name,
                    to_component, to_port, var_name
                ]
            )
    
    # Connect the connectors
    from_connector.connect_to(to_connector)
    return true

func initialize() -> bool:
    # Collect all variables and equations
    for component in components.values():
        # Add variables to equation system
        for var_name in component.variables:
            var var_obj = component.get_variable(var_name)
            equation_system.add_variable(var_name, var_obj.kind)
        
        # Add equations to equation system
        for eq in component.get_equations():
            equation_system.add_equation(eq)
        
        # Add initial equations
        for eq in component.get_initial_equations():
            equation_system.add_equation(eq)
    
    # Solve initialization problem
    is_initialized = equation_system.solve_initialization()
    return is_initialized

func simulate(duration: float) -> void:
    if not is_initialized:
        push_error("System not initialized")
        return
    
    var steps = int(duration / dt)
    for i in range(steps):
        time += dt
        equation_system.solve()

func _on_component_state_changed(var_name: String, value: float, comp_name: String) -> void:
    emit_signal("state_changed", comp_name, var_name, value)

func _on_component_parameter_changed(param_name: String, value: Variant, comp_name: String) -> void:
    emit_signal("parameter_changed", comp_name, param_name, value)

func _on_component_event_triggered(event_name: String, data: Dictionary, comp_name: String) -> void:
    emit_signal("event_triggered", comp_name, event_name, data)

func _to_string() -> String:
    var result = "ModelicaSystem:\n"
    result += "  Time: %f\n" % time
    result += "  Components:\n"
    for comp_name in components:
        result += "    %s\n" % comp_name
    return result 