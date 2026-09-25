package AsyncTwinAnalysis {
    private import ScalarValues::Real;

    requirement def PowerBudget {
        attribute minimumWatts : Real = 45.0;
    }

    part def Rover;
    requirement powerBudget : PowerBudget {
        subject rover : Rover;
    }
    requirement otherBudget : PowerBudget {
        subject rover : Rover;
    }
    verification def VerifyPowerBudget {
        subject rover : Rover;
        objective {
            verify powerBudget;
        }
    }
}
