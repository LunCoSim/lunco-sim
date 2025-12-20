# core/simulation/PhysioNetworkBridge.gd
extends Node

class_name PhysioNetworkBridge

# Define the data structure for physio-network metrics
class PhysioNetworkMetrics:
    var hrv: float
    var avgCoherence: float
    var correlation: float
    var impactFactor: float
    var timestamp: int

    func _init(p_hrv: float, p_avgCoherence: float, p_correlation: float, p_impactFactor: float, p_timestamp: int):
        hrv = p_hrv
        avgCoherence = p_avgCoherence
        correlation = p_correlation
        impactFactor = p_impactFactor
        timestamp = p_timestamp

var physioHistory: Array[PhysioNetworkMetrics] = [] # Ring buffer
var network_simulator = NetworkSimulator.new()

# HRV influence calculator
func getHRVInfluence(hrv: float) -> float:
    # 30-100 ms  →  0.8-1.2  multiplicative factor
    var clamped = clamp(hrv, 30.0, 100.0)
    return 0.8 + ((clamped - 30.0) / 70.0) * 0.4

# Pearson r (quick util)
func calcCorrelation(history: Array[PhysioNetworkMetrics]) -> float:
    if history.size() < 3:
        return 0.0
    var n = history.size()
    var sumH = 0.0
    var sumC = 0.0
    var sumH2 = 0.0
    var sumC2 = 0.0
    var sumHC = 0.0
    for m in history:
        sumH += m.hrv
        sumC += m.avgCoherence
        sumH2 += m.hrv * m.hrv
        sumC2 += m.avgCoherence * m.avgCoherence
        sumHC += m.hrv * m.avgCoherence
    var num = n * sumHC - sumH * sumC
    var den = sqrt((n * sumH2 - sumH * sumH) * (n * sumC2 - sumC * sumC))
    return den if den != 0.0 else 0.0

# New endpoint
func getPhysioNetworkMetrics() -> PhysioNetworkMetrics:
    # Mock data for now
    var vitals = {"hrv": 60.0 + randf_range(-5, 5)}
    var influence = getHRVInfluence(vitals.hrv)

    network_simulator.updateWithPhysioInfluence(influence)
    var coherenceMetrics = {"avgCoherence": network_simulator.get_avg_coherence()}

    var m = PhysioNetworkMetrics.new(
        vitals.hrv,
        coherenceMetrics.avgCoherence,
        calcCorrelation(physioHistory),
        influence,
        Time.get_unix_time_from_system()
    )

    physioHistory.push_back(m)
    if physioHistory.size() > 60: # 60 s sliding window
        physioHistory.pop_front()

    return m
