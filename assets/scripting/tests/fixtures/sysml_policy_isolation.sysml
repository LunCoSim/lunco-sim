package PolicyIsolation {
    private import ScalarValues::Real;

    requirement def MassBudget {
        doc /* The analysis policy must still be able to inspect this source. */
        attribute limit : Real = 20.0;
    }

    part def Rover;
    requirement massBudget : MassBudget;
    requirement sourcedBudget : MassBudget { subject rover : Rover; }
    requirement emptySourceBudget : MassBudget { subject rover : Rover; }
    requirement unlinkedBudget : MassBudget { subject rover : Rover; }

    part def EvidenceSource { attribute locator : ScalarValues::String; }
    part def RequirementEvidence {
        ref targetRequirement : SysML::RequirementUsage;
        ref part sources : EvidenceSource[0..*];
    }
    part validSource : EvidenceSource { attribute locator = "https://example.org/mass-budget"; }
    part emptySource : EvidenceSource { attribute locator = ""; }
    part validEvidence : RequirementEvidence {
        ref targetRequirement = sourcedBudget;
        ref sources = (validSource);
    }
    part emptyEvidence : RequirementEvidence {
        ref targetRequirement = emptySourceBudget;
        ref sources = (emptySource);
    }
    part unlinkedEvidence : RequirementEvidence {
        ref targetRequirement = unlinkedBudget;
    }
}
