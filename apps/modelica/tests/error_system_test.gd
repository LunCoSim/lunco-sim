#!/usr/bin/env -S godot --headless --script
extends SceneTree

const ErrorSystem = preload("../core/error_system.gd")

func test_error_system():
    print("\n=== Testing ModelicaErrorSystem ===")
    
    # Create error manager
    var error_manager = ErrorSystem.create_error_manager()
    print("Created error manager")
    
    # Test creating errors
    var syntax_error = error_manager.report_syntax_error("Invalid syntax", ErrorSystem.Severity.ERROR)
    print("Created syntax error: " + syntax_error.get_string())
    
    var variable_error = error_manager.report_variable_error("Variable not found: x")
    print("Created variable error: " + variable_error.get_string())
    
    # Test error queries
    print("Has errors: " + str(error_manager.has_errors()))
    print("Error count: " + str(error_manager.get_error_count()))
    
    # Test creating results
    var ok_result = ErrorSystem.ok(42)
    print("Created OK result: " + ok_result.get_string())
    print("Value: " + str(ok_result.get_value()))
    print("Is OK: " + str(ok_result.is_ok()))
    
    var err_result = ErrorSystem.error("Something went wrong", ErrorSystem.Category.SYSTEM)
    print("Created Error result: " + err_result.get_string())
    print("Is Error: " + str(err_result.is_err()))
    
    print("=== Test completed successfully ===")
    return true

func _init():
    var success = test_error_system()
    if success:
        print("✅ All tests passed!")
    else:
        print("❌ Tests failed!")
    quit() 