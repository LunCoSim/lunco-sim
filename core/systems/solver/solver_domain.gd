class_name SolverDomain
extends RefCounted

const LIQUID = "Liquid"
const GAS = "Gas"
const SOLID = "Solid"
const ELECTRICAL = "Electrical"
const THERMAL = "Thermal"

static func is_valid(domain: StringName) -> bool:
	return domain in [LIQUID, GAS, SOLID, ELECTRICAL, THERMAL]
