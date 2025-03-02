#!/usr/bin/env -S godot --headless --script
extends SceneTree

class TestASTNode extends "res://apps/modelica/tests/base_test.gd":
	const ASTNode = preload("res://apps/modelica/core/ast_node.gd")
	
	# Setup variable to hold instances we'll use in multiple tests
	var root_node
	var child_node
	var complex_tree
	
	func setup():
		# Create fresh node instances for each test
		root_node = ASTNode.new(ASTNode.NodeType.ROOT, "root")
		child_node = null
		complex_tree = null
	
	func teardown():
		# Ensure proper cleanup
		root_node = null
		child_node = null
		complex_tree = null
	
	func test_initialization():
		# Test basic initialization
		var node = ASTNode.new(ASTNode.NodeType.MODEL, "TestModel", {"line": 10, "column": 5})
		
		assert_equal(node.type, ASTNode.NodeType.MODEL, "Node type should be MODEL")
		assert_equal(node.value, "TestModel", "Node value should be TestModel")
		assert_equal(node.source_location.line, 10, "Source line should be 10")
		assert_equal(node.source_location.column, 5, "Source column should be 5")
		assert_equal(node.children.size(), 0, "New node should have no children")
		assert_null(node.parent, "New node should have no parent")
	
	func test_default_initialization():
		# Test initialization with defaults
		var node = ASTNode.new()
		
		assert_equal(node.type, ASTNode.NodeType.UNKNOWN, "Default node type should be UNKNOWN")
		assert_null(node.value, "Default node value should be null")
		assert_equal(node.source_location.line, 0, "Default source line should be 0")
		assert_equal(node.source_location.column, 0, "Default source column should be 0")
	
	func test_add_child():
		# Test adding a child node
		child_node = ASTNode.new(ASTNode.NodeType.PARAMETER, "param1")
		root_node.add_child(child_node)
		
		assert_equal(root_node.children.size(), 1, "Root should have one child")
		assert_equal(root_node.children[0], child_node, "Child should be in parent's children array")
		assert_equal(child_node.parent, root_node, "Parent reference should be set correctly")
	
	func test_error_handling():
		# Test error handling methods
		var node = ASTNode.new(ASTNode.NodeType.VARIABLE, "x")
		
		# Initially no errors
		assert_false(node.has_errors, "Node should not have errors initially")
		assert_equal(node.errors.size(), 0, "Errors array should be empty initially")
		
		# Add an error
		node.add_error("Test error message")
		
		assert_true(node.has_errors, "Node should have errors after adding one")
		assert_equal(node.errors.size(), 1, "Errors array should have one item")
		assert_equal(node.errors[0].message, "Test error message", "Error message should be correct")
		assert_equal(node.errors[0].type, "error", "Error type should be 'error'")
	
	func test_error_propagation():
		# Test error propagation to parent nodes
		var parent = ASTNode.new(ASTNode.NodeType.MODEL, "Model")
		var child = ASTNode.new(ASTNode.NodeType.PARAMETER, "param")
		
		parent.add_child(child)
		child.add_error("Child error")
		
		assert_true(child.has_errors, "Child should have errors")
		assert_true(parent.has_errors, "Parent should have propagated errors")
		assert_equal(parent.errors.size(), 1, "Parent should have the same number of errors")
		assert_equal(parent.errors[0].message, "Child error", "Error message should propagate")
	
	func test_get_root():
		# Test get_root() method
		var level1 = ASTNode.new(ASTNode.NodeType.PACKAGE, "pack")
		var level2 = ASTNode.new(ASTNode.NodeType.MODEL, "model")
		var level3 = ASTNode.new(ASTNode.NodeType.COMPONENT, "comp")
		
		level1.add_child(level2)
		level2.add_child(level3)
		
		assert_equal(level3.get_root(), level1, "Level3 root should be level1")
		assert_equal(level2.get_root(), level1, "Level2 root should be level1")
		assert_equal(level1.get_root(), level1, "Level1 root should be itself")
	
	func test_find_child_by_name():
		# Test find_child_by_name method
		var parent = ASTNode.new(ASTNode.NodeType.MODEL, "TestModel")
		var child1 = ASTNode.new(ASTNode.NodeType.PARAMETER, "p1")
		var child2 = ASTNode.new(ASTNode.NodeType.PARAMETER, "p2")
		
		parent.add_child(child1)
		parent.add_child(child2)
		
		assert_equal(parent.find_child_by_name("p1"), child1, "Should find child1 by name")
		assert_equal(parent.find_child_by_name("p2"), child2, "Should find child2 by name")
		assert_null(parent.find_child_by_name("p3"), "Should return null for nonexistent name")
	
	func test_get_full_name():
		# Test get_full_name method for qualified names
		var level1 = ASTNode.new(ASTNode.NodeType.PACKAGE, "Modelica")
		var level2 = ASTNode.new(ASTNode.NodeType.PACKAGE, "Mechanics")
		var level3 = ASTNode.new(ASTNode.NodeType.MODEL, "Mass")
		
		level1.add_child(level2)
		level2.add_child(level3)
		
		assert_equal(level3.get_full_name(), "Modelica.Mechanics.Mass", 
			"Full name should be dot-separated path")
		assert_equal(level2.get_full_name(), "Modelica.Mechanics", 
			"Partial path should be correct")
		assert_equal(level1.get_full_name(), "Modelica", 
			"Root name should be just the name")
	
	func test_type_check_methods():
		# Test the is_* methods
		var model_node = ASTNode.new(ASTNode.NodeType.MODEL, "Model")
		var eq_node = ASTNode.new(ASTNode.NodeType.EQUATION, "eq")
		var num_node = ASTNode.new(ASTNode.NodeType.NUMBER, "42")
		var type_node = ASTNode.new(ASTNode.NodeType.TYPE_DEFINITION, "Type")
		
		# Test is_definition()
		assert_true(model_node.is_definition(), "MODEL should be a definition")
		assert_false(eq_node.is_definition(), "EQUATION should not be a definition")
		
		# Test is_equation()
		assert_true(eq_node.is_equation(), "EQUATION should be an equation")
		assert_false(model_node.is_equation(), "MODEL should not be an equation")
		
		# Test is_expression()
		assert_true(num_node.is_expression(), "NUMBER should be an expression")
		assert_false(model_node.is_expression(), "MODEL should not be an expression")
		
		# Test is_type()
		assert_true(type_node.is_type(), "TYPE_DEFINITION should be a type")
		assert_false(model_node.is_type(), "MODEL should not be a type")
	
	func test_complex_tree_operations():
		# Test operations on a more complex tree
		complex_tree = ASTNode.new(ASTNode.NodeType.ROOT, "root")
		
		var model = ASTNode.new(ASTNode.NodeType.MODEL, "TestModel")
		complex_tree.add_child(model)
		
		var param1 = ASTNode.new(ASTNode.NodeType.PARAMETER, "p1")
		param1.visibility = "public"
		model.add_child(param1)
		
		var param2 = ASTNode.new(ASTNode.NodeType.PARAMETER, "p2")
		param2.visibility = "protected"
		model.add_child(param2)
		
		var eq = ASTNode.new(ASTNode.NodeType.EQUATION, "eq")
		model.add_child(eq)
		
		# Test tree structure
		assert_equal(complex_tree.children.size(), 1, "Root should have one child")
		assert_equal(model.children.size(), 3, "Model should have three children")
		
		# Test finding by type
		var params = []
		for child in model.children:
			if child.type == ASTNode.NodeType.PARAMETER:
				params.append(child)
		
		assert_equal(params.size(), 2, "Should find two parameters")
		
		# Test other properties
		assert_equal(param1.visibility, "public", "Visibility should be set")
		assert_equal(param2.visibility, "protected", "Visibility should be set")

func _init():
	var direct_execution = true
	
	# Check if we're being run directly or via the test runner
	for arg in OS.get_cmdline_args():
		if arg.ends_with("run_tests.gd"):
			direct_execution = false
			break
	
	if direct_execution:
		print("\nRunning TestASTNode...")
		var test = TestASTNode.new()
		test.run_tests()
		quit() 