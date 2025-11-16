# components/dashboard/tabs/PhysioNetworkBridge.gd
extends Control

@onready var loading_label = $LoadingLabel
@onready var correlation_value = $MainGrid/ChartCard/ChartVBox/MetricsGrid/CorrelationValue
@onready var impact_factor_value = $MainGrid/ChartCard/ChartVBox/MetricsGrid/ImpactFactorValue
@onready var hrv_value = $MainGrid/GaugeCard/GaugeVBox/HRVValue
@onready var avg_coherence_value = $MainGrid/GaugeCard/GaugeVBox/AvgCoherenceValue
@onready var gauge = $MainGrid/GaugeCard/GaugeVBox/Gauge
@onready var chart = $MainGrid/ChartCard/ChartVBox/Chart
@onready var timer = $Timer

var history = []
var hrv_line = Line2D.new()
var coherence_line = Line2D.new()

func _ready():
    timer.timeout.connect(_on_Timer_timeout)
    loading_label.show()
    $MainGrid.hide()

    hrv_line.width = 2
    hrv_line.default_color = Color(0.5, 0.5, 1.0)
    chart.add_child(hrv_line)

    coherence_line.width = 2
    coherence_line.default_color = Color(0.5, 1.0, 0.5)
    chart.add_child(coherence_line)

func _on_Timer_timeout():
    var m = PhysioNetworkBridge.getPhysioNetworkMetrics()

    if m:
        if loading_label.visible:
            loading_label.hide()
            $MainGrid.show()

        correlation_value.text = "%.2f" % m.correlation
        impact_factor_value.text = "%.2f" % m.impactFactor
        hrv_value.text = "%d ms" % m.hrv
        avg_coherence_value.text = "%.1f %%" % (m.avgCoherence * 100)

        var boostPct = round(((m.impactFactor - 0.8) / 0.4) * 100)
        gauge.value = boostPct

        history.push_back(m)
        if history.size() > 60:
            history.pop_front()

        update_chart()

        if m.correlation < 0.5:
            print("[Bridge] Low correlation: %.2f" % m.correlation)

func update_chart():
    hrv_line.clear_points()
    coherence_line.clear_points()

    var chart_width = chart.size.x
    var chart_height = chart.size.y
    var num_points = history.size()
    var point_spacing = chart_width / (num_points - 1) if num_points > 1 else 0

    for i in range(num_points):
        var m = history[i]
        var x = i * point_spacing

        var hrv_y = chart_height - (m.hrv - 30) / (100 - 30) * chart_height
        hrv_line.add_point(Vector2(x, hrv_y))

        var coherence_y = chart_height - m.avgCoherence * chart_height
        coherence_line.add_point(Vector2(x, coherence_y))
