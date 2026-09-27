// UCUM-compatible unit vocabulary for authored policy.
//
// Rust receives only resolved EngineeringUnit values. This library is the
// replaceable vocabulary/catalog edge: a Twin may import it, extend it with
// project units, or replace it with definitions resolved from its SysML
// library. No relation policy belongs here.

fn _dimension(l, m, t, i, temperature, amount, luminous) {
    [l, m, t, i, temperature, amount, luminous]
}

fn _unit(symbol, dimensions, scale, offset, exact) {
    engineering_unit_with_exactness(symbol, dimensions, scale, offset, exact)
}

fn ucum_unit(symbol) {
    if symbol == "1" { return _unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "m" { return _unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "cm" { return _unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 0.01, 0.0, true); }
    if symbol == "mm" { return _unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 0.001, 0.0, true); }
    if symbol == "km" { return _unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 1000.0, 0.0, true); }
    if symbol == "s" { return _unit(symbol, _dimension(0, 0, 1, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "ms" { return _unit(symbol, _dimension(0, 0, 1, 0, 0, 0, 0), 0.001, 0.0, true); }
    if symbol == "kg" { return _unit(symbol, _dimension(0, 1, 0, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "g" { return _unit(symbol, _dimension(0, 1, 0, 0, 0, 0, 0), 0.001, 0.0, true); }
    if symbol == "K" { return _unit(symbol, _dimension(0, 0, 0, 0, 1, 0, 0), 1.0, 0.0, true); }
    if symbol == "Cel" || symbol == "degC" || symbol == "°C" {
        return _unit("Cel", _dimension(0, 0, 0, 0, 1, 0, 0), 1.0, 273.15, true);
    }
    if symbol == "A" { return _unit(symbol, _dimension(0, 0, 0, 1, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "mol" { return _unit(symbol, _dimension(0, 0, 0, 0, 0, 1, 0), 1.0, 0.0, true); }
    if symbol == "cd" { return _unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 1), 1.0, 0.0, true); }
    if symbol == "N" { return _unit(symbol, _dimension(1, 1, -2, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "Pa" { return _unit(symbol, _dimension(-1, 1, -2, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "J" { return _unit(symbol, _dimension(2, 1, -2, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "W" { return _unit(symbol, _dimension(2, 1, -3, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "Hz" { return _unit(symbol, _dimension(0, 0, -1, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "rad" { return _unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 0), 1.0, 0.0, true); }
    if symbol == "deg" { return _unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 0), 0.017453292519943295, 0.0, false); }
    ()
}

fn quantity_from_ucum(value, symbol) {
    let unit = ucum_unit(symbol);
    if unit == () { return (); }
    quantity(value, unit)
}

fn convert_ucum(value, from, to) {
    let from_unit = ucum_unit(from);
    let to_unit = ucum_unit(to);
    if from_unit == () || to_unit == () { return (); }
    engineering_convert_or_none(value, from_unit, to_unit)
}

fn linear_scale(symbol) {
    let unit = ucum_unit(symbol);
    if unit == () { return (); }
    engineering_unit_scale(unit)
}
