package SysmlRequirementConstraint {
    private import ScalarValues::Real;

    part def PayloadItem;

    requirement def PayloadCapacity {
        doc /* Payload mass does not exceed the qualified capacity. */
        require PayloadWithinCapacity;
    }
    requirement payloadCapacity : PayloadCapacity {
        subject payload : PayloadItem;
    }

    constraint def PayloadWithinCapacity {
        in payloadKg : Real;
        in capacityKg : Real;
        payloadKg >= 0.0 and payloadKg <= capacityKg;
    }

    constraint def UnrelatedCapacityCheck {
        in payloadKg : Real;
        in capacityKg : Real;
        payloadKg >= 0.0 and payloadKg <= capacityKg;
    }

    verification def VerifyPayloadCapacity {
        subject payload : PayloadItem;
        objective {
            verify payloadCapacity;
        }
    }
}
