package PolicyIsolation {
    private import ScalarValues::Real;

    requirement def MassBudget {
        doc /* The analysis policy must still be able to inspect this source. */
        attribute limit : Real = 20.0;
    }

    part def Rover;
    requirement massBudget : MassBudget;
}
