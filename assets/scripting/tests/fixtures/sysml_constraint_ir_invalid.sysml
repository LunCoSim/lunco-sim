package ConstraintIrDiagnostics {
    private import ScalarValues::Boolean;
    private import ScalarValues::Real;

    constraint def TypeMismatch {
        in enabled : Boolean;
        in magnitude : Real;
        enabled + magnitude == 1.0;
    }
}
