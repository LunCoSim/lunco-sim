package AsyncTwinAnalysis {
    private import ScalarValues::String;
    private import SI::W;
    private import ISQ::PowerValue;

    attribute def SourceLocator :> String;
    enum def EvidenceSourceRole { Reference; }

    part def EvidenceSource {
        attribute role : EvidenceSourceRole;
        attribute locator : SourceLocator;
    }

    part def RequirementEvidence {
        ref targetRequirement : SysML::RequirementUsage;
        ref part sources : EvidenceSource[0..*];
    }

    part def Rover;
    part rover : Rover;

    requirement def PowerBudget {
        doc /* The rover shall deliver continuous electrical power greater than or equal to minimumPower under the nominal test condition. */
        attribute minimumPower : PowerValue = 45.0[W];
        require MinimumOutputPower;
    }

    constraint def MinimumOutputPower {
        in deliveredPower : PowerValue;
        in minimumPower : PowerValue;
        deliveredPower >= minimumPower;
    }

    satisfy PowerBudget by rover;

    requirement powerBudget : PowerBudget {
        doc /* Verify the nominal electrical output of the rover. */
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

    part power_budget_source : EvidenceSource {
        attribute role : EvidenceSourceRole = "Reference";
        attribute locator : SourceLocator = "twin://sysml_async_analysis/power_budget_source.md";
    }
    part evidence_power_budget : RequirementEvidence {
        ref targetRequirement = powerBudget;
        ref sources = (power_budget_source);
    }
    part evidence_other_budget : RequirementEvidence {
        ref targetRequirement = otherBudget;
        ref sources = (power_budget_source);
    }
}