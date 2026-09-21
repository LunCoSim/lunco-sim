package NativeValues {
    private import VectorValues::CartesianThreeVectorValue;
    private import ScalarValues::Real;
    private import ScalarValues::String;
    private import SI::m;
    private import ISQ::LengthValue;

    attribute def Position :> CartesianThreeVectorValue;
    attribute def RailCoordinate :> Real;

    enum def LanderMode {
        Landed;
    }

    part def Lander {
        attribute position : CartesianThreeVectorValue = (1.0000000000000002, -2.0, 3.5);
        attribute positionAlias : Position = (4.5, -6.0, 7.25);
        attribute positionStations : Position[2] =
            ((1.0, 2.0, 3.0), (-4.0, 5.0, -6.0));
        attribute calibration : Real[3] = (0.25, 0.5, 0.75);
        attribute stationNames : String[2] = ("fore", "aft");
        attribute railLength : Real = 2.6;
        attribute railOffset : RailCoordinate = 1.25;
        attribute railStations : Real[7] = (0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6);
        attribute skirtLength : LengthValue = 0.75[m];
        attribute mode : LanderMode = "Landed";
    }

    requirement def RailLengthRequirement {
        doc /* The lander's rail datum bounds the deployed support footprint. */
    }
    requirement railLengthRequirement : RailLengthRequirement {
        subject vehicle : Lander;
    }

    verification def VerifyRailLength {
        subject vehicle : Lander;
        verify railLengthRequirement;
    }
}
