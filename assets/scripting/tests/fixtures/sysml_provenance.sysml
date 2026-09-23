package EvidenceCatalog {
    private import ScalarValues::String;

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

    requirement def ProbeRequirement;
    requirement <'GR-001'> gr001 : ProbeRequirement;

    part source_griffin_study : EvidenceSource {
        attribute role : EvidenceSourceRole = "Reference";
        attribute locator : SourceLocator = "https://example.org/griffin-study";
    }

    part evidence_gr001 : RequirementEvidence {
        ref targetRequirement = gr001;
        ref sources = (source_griffin_study);
    }
}
