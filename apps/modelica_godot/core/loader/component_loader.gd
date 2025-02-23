@tool
extends BaseLoader
class_name ComponentLoader

signal component_loaded(component_name: String)
signal component_loading_error(component_name: String, error: String)

var _components: Dictionary = {}  # name -> component data
var _connectors: Dictionary = {}  # name -> connector data

func load_component_file(path: String) -> Dictionary:
    print("Loading component from: ", path)
    
    # Check if already loaded
    if _components.has(path):
        return _components[path]
    
    # Load and parse file
    var model_data = load_file(path)
    if not validate_model_data(model_data):
        return {}
    
    if not model_data.is_empty():
        # Check if this is a connector definition
        if model_data.get("type", "") == "connector":
            _process_connector(model_data)
        
        _components[path] = model_data
        print("Loaded component: ", model_data.get("name", ""), " of type: ", model_data.get("type", ""))
        emit_signal("component_loaded", model_data.get("name", ""))
        
    return model_data

func _process_connector(connector_data: Dictionary) -> void:
    var name = connector_data.get("name", "")
    if name.is_empty():
        push_error("Connector has no name")
        return
        
    var processed_data = {
        "name": name,
        "type": "connector",
        "variables": {},
        "flow_variables": [],  # Track flow variables separately
        "potential_variables": []  # Track potential variables
    }
    
    # Process variables
    for var_data in connector_data.get("variables", []):
        var var_name = var_data.get("name", "")
        if var_name.is_empty():
            continue
            
        processed_data.variables[var_name] = var_data
        
        # Track flow and potential variables
        if var_data.get("flow", false):
            processed_data.flow_variables.append(var_name)
        else:
            processed_data.potential_variables.append(var_name)
    
    _connectors[name] = processed_data
    print("Processed connector: ", name, " with flow variables: ", processed_data.flow_variables)

func create_component(model_data: Dictionary) -> ModelicaComponent:
    var component = ModelicaComponent.new(model_data.get("name", ""), model_data.get("description", ""))
    
    # Load parameters
    for param in model_data.get("parameters", []):
        var value = _convert_parameter_value(param)
        component.add_parameter(param.get("name", ""), value)
    
    # Load variables
    for var_data in model_data.get("variables", []):
        if var_data.get("flow", false):
            # Flow variables become connectors
            var connector_name = var_data.get("name", "")
            var connector_type = _get_connector_type(var_data.get("type", ""))
            component.add_connector(connector_name, connector_type)
        else:
            # Regular variables
            component.add_variable(var_data.get("name", ""))
    
    # Load equations
    for eq in model_data.get("equations", []):
        component.add_equation(eq)
    
    # Load annotations
    component.annotations = model_data.get("annotations", {}).duplicate()
    
    return component

func _get_connector_type(type_str: String) -> ModelicaConnector.ConnectorType:
    match type_str.to_lower():
        "mechanical":
            return ModelicaConnector.ConnectorType.MECHANICAL
        "electrical":
            return ModelicaConnector.ConnectorType.ELECTRICAL
        "thermal":
            return ModelicaConnector.ConnectorType.THERMAL
        "fluid":
            return ModelicaConnector.ConnectorType.FLUID
        "signal":
            return ModelicaConnector.ConnectorType.SIGNAL
        _:
            return ModelicaConnector.ConnectorType.NONE

func get_component(path: String) -> Dictionary:
    return _components.get(path, {})

func has_component(name: String) -> bool:
    # First check connectors
    if _connectors.has(name):
        return true
        
    # Then check components
    for path in _components:
        var model = _components[path]
        if model.get("name", "") == name:
            return true
    return false

func get_component_by_name(name: String) -> Dictionary:
    # First check connectors
    if _connectors.has(name):
        return _connectors[name]
        
    # Then check components
    for path in _components:
        var model = _components[path]
        if model.get("name", "") == name:
            return model
    return {}

func get_connector_info(name: String) -> Dictionary:
    return _connectors.get(name, {})

func is_flow_variable(connector_name: String, variable_name: String) -> bool:
    var connector = _connectors.get(connector_name, {})
    return variable_name in connector.get("flow_variables", []) 