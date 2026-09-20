package EvidenceCatalog {
    private import ScalarValues::String;

    part def RequirementEvidence {
        attribute requirementId : String = "GR-001";
        attribute qualifiedRequirement : String = "Griffin::GR001";
        attribute sourceReference : String = "https://example.org/griffin-study";
        attribute rationale : String = "sets a traceable first-pass geometry datum";
        attribute status : String = "study_assumption";
    }
}
