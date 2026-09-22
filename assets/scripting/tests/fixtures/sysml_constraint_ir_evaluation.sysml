package ConstraintIrEvaluation {
    private import ScalarValues::Real;

    constraint def SumEquals {
        in left : Real;
        in right : Real;
        left + right == 3.0;
    }

    constraint def BooleanFalse {
        false;
    }
}
