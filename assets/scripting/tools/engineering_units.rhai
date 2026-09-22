// UCUM-compatible unit vocabulary for authored policy.
//
// Rust receives only resolved EngineeringUnit values. This library is the
// replaceable vocabulary/catalog edge: a Twin may import it, extend it with
// project units, or replace it with definitions resolved from its SysML
// library. No relation policy belongs here.

fn _dimension(l, m, t, i, temperature, amount, luminous) {
    [l, m, t, i, temperature, amount, luminous]
}

fn ucum_unit(symbol) {
    if symbol == "1" { return engineering_unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "m" { return engineering_unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "cm" { return engineering_unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 0.01, 0.0); }
    if symbol == "mm" { return engineering_unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 0.001, 0.0); }
    if symbol == "km" { return engineering_unit(symbol, _dimension(1, 0, 0, 0, 0, 0, 0), 1000.0, 0.0); }
    if symbol == "s" { return engineering_unit(symbol, _dimension(0, 0, 1, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "ms" { return engineering_unit(symbol, _dimension(0, 0, 1, 0, 0, 0, 0), 0.001, 0.0); }
    if symbol == "kg" { return engineering_unit(symbol, _dimension(0, 1, 0, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "g" { return engineering_unit(symbol, _dimension(0, 1, 0, 0, 0, 0, 0), 0.001, 0.0); }
    if symbol == "K" { return engineering_unit(symbol, _dimension(0, 0, 0, 0, 1, 0, 0), 1.0, 0.0); }
    if symbol == "Cel" || symbol == "degC" || symbol == "°C" {
        return engineering_unit("Cel", _dimension(0, 0, 0, 0, 1, 0, 0), 1.0, 273.15);
    }
    if symbol == "A" { return engineering_unit(symbol, _dimension(0, 0, 0, 1, 0, 0, 0), 1.0, 0.0); }
    if symbol == "mol" { return engineering_unit(symbol, _dimension(0, 0, 0, 0, 0, 1, 0), 1.0, 0.0); }
    if symbol == "cd" { return engineering_unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 1), 1.0, 0.0); }
    if symbol == "N" { return engineering_unit(symbol, _dimension(1, 1, -2, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "Pa" { return engineering_unit(symbol, _dimension(-1, 1, -2, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "J" { return engineering_unit(symbol, _dimension(2, 1, -2, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "W" { return engineering_unit(symbol, _dimension(2, 1, -3, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "Hz" { return engineering_unit(symbol, _dimension(0, 0, -1, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "rad" { return engineering_unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 0), 1.0, 0.0); }
    if symbol == "deg" { return engineering_unit(symbol, _dimension(0, 0, 0, 0, 0, 0, 0), 0.017453292519943295, 0.0); }
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
