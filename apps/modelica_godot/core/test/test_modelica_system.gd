extends SceneTree

class_name TestModelicaSystem

const ModelicaVariable = preload("res://apps/modelica_godot/core/modelica_variable.gd")
const ModelicaComponent = preload("res://apps/modelica_godot/core/modelica_component.gd")
const ModelicaConnector = preload("res://apps/modelica_godot/core/modelica_connector.gd")
const ModelicaEquationSystem = preload("res://apps/modelica_godot/core/equation_system.gd")

# Test framework setup
var tests_run := 0
var tests_passed := 0
var current_test := ""

func _init():
    print("\nRunning Modelica System Tests...")
    _run_all_tests()
    print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])

func _run_all_tests() -> void:
    _test_variable_creation()
    _test_component_creation()
    _test_connector_creation()
    _test_equation_system()
    _test_connection()

func _start_test(test_name: String) -> void:
    current_test = test_name
    tests_run += 1
    print("\nRunning test: " + test_name)

func _assert(condition: bool, message: String) -> void:
    if condition:
        tests_passed += 1
        print("  ✓ " + message)
    else:
        print("  ✗ " + message)
        push_error("Test failed: " + current_test + " - " + message)

func _test_variable_creation() -> void:
    _start_test("Variable Creation")
    
    var var_obj = ModelicaVariable.new("test_var")
    _assert(var_obj.get_declaration("test_var") != null, "Basic variable declaration created")
    _assert(var_obj.kind == ModelicaVariable.VariableKind.REGULAR, "Variable kind set correctly")
    _assert(var_obj.value == 0.0, "Default value set correctly")
    
    var param_obj = ModelicaVariable.new("test_param", ModelicaVariable.VariableKind.PARAMETER, 5.0)
    _assert(param_obj.is_parameter(), "Parameter type set correctly")
    _assert(param_obj.value == 5.0, "Parameter value set correctly")
    
    var state_var = ModelicaVariable.new("pos", ModelicaVariable.VariableKind.STATE, 1.0)
    _assert(state_var.is_state_variable(), "State variable type set correctly")
    _assert(state_var.value == 1.0, "State variable value set correctly")

func _test_component_creation() -> void:
    _start_test("Component Creation")
    
    var comp = ModelicaComponent.new("test_component", "A test component")
    _assert(comp.get_declaration("test_component") != null, "Component declaration created")
    
    var var_obj = comp.add_variable("x", ModelicaVariable.VariableKind.REGULAR, 1.0)
    _assert(comp.get_variable("x") == var_obj, "Variable added to component")
    _assert(var_obj.value == 1.0, "Variable value set correctly")
    
    var state_var = comp.add_state_variable("pos", 2.0)
    _assert(comp.get_variable("pos") == state_var, "State variable added to component")
    _assert(comp.get_variable("der(pos)") != null, "Derivative variable created")
    
    var port_var = comp.add_variable("port.force", ModelicaVariable.VariableKind.FLOW, 0.0)
    _assert(comp.get_variable("port.force") == port_var, "Port variable added")
    _assert(comp.get_connector("port") != null, "Connector created for port variable")

func _test_connector_creation() -> void:
    _start_test("Connector Creation")
    
    var conn = ModelicaConnector.new("test_port")
    _assert(conn.get_declaration("test_port") != null, "Connector declaration created")
    
    var flow_var = conn.add_variable("flow", ModelicaVariable.VariableKind.FLOW)
    _assert(conn.get_variable("flow") == flow_var, "Flow variable added to connector")
    _assert(flow_var.is_flow_variable(), "Flow variable type set correctly")
    
    var potential_var = conn.add_variable("potential")
    _assert(conn.get_variable("potential") == potential_var, "Potential variable added to connector")

func _test_equation_system() -> void:
    _start_test("Equation System")
    
    var sys = ModelicaEquationSystem.new()
    
    # Create a simple mass-spring system
    var mass = ModelicaComponent.new("mass")
    mass.add_state_variable("pos", 1.0)  # Initial position = 1
    mass.add_state_variable("vel", 0.0)  # Initial velocity = 0
    mass.add_parameter("m", ModelicaVariable.VariableKind.PARAMETER, 1.0)  # Mass = 1kg
    mass.add_equation("der(pos) = vel")
    mass.add_equation("der(vel) = -k * pos / m")  # F = -kx
    
    # Add spring constant as a parameter
    mass.add_parameter("k", ModelicaVariable.VariableKind.PARAMETER, 1.0)  # Spring constant = 1 N/m
    
    sys.add_component(mass)
    
    # Initialize and solve one step
    _assert(sys.solve_initialization(), "System initialization successful")
    
    # Solve a few steps and check energy conservation
    var initial_energy = 0.5 * 1.0 * pow(0.0, 2) + 0.5 * 1.0 * pow(1.0, 2)  # 1/2*m*v^2 + 1/2*k*x^2
    
    for i in range(10):
        _assert(sys.solve_step(), "Simulation step %d successful" % i)
        
        var pos = sys.get_variable_value("mass.pos")
        var vel = sys.get_variable_value("mass.vel")
        var current_energy = 0.5 * 1.0 * pow(vel, 2) + 0.5 * 1.0 * pow(pos, 2)
        
        # Check energy conservation (with some numerical tolerance)
        _assert(abs(current_energy - initial_energy) < 0.01, "Energy conserved at step %d" % i)

func _test_connection() -> void:
    _start_test("Connections")
    
    var sys = ModelicaEquationSystem.new()
    
    # Create two masses connected by a spring
    var mass1 = ModelicaComponent.new("mass1")
    var mass2 = ModelicaComponent.new("mass2")
    
    # Add ports to masses
    mass1.add_variable("port.force", ModelicaVariable.VariableKind.FLOW)
    mass1.add_variable("port.pos", ModelicaVariable.VariableKind.REGULAR)
    
    mass2.add_variable("port.force", ModelicaVariable.VariableKind.FLOW)
    mass2.add_variable("port.pos", ModelicaVariable.VariableKind.REGULAR)
    
    # Connect the ports
    var conn1 = mass1.get_connector("port")
    var conn2 = mass2.get_connector("port")
    sys.connect(conn1, conn2)
    
    # Check connection equations
    _assert(sys.runtime_system.equations.size() > 0, "Connection equations generated") 