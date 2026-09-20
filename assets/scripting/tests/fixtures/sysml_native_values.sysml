package NativeValues {
    private import VectorValues::CartesianThreeVectorValue;

    attribute def Position :> CartesianThreeVectorValue;

    part def Lander {
        attribute position : CartesianThreeVectorValue = (1.0000000000000002, -2.0, 3.5);
        attribute positionAlias : Position = (4.5, -6.0, 7.25);
        attribute positionStations : Position[2] =
            ((1.0, 2.0, 3.0), (-4.0, 5.0, -6.0));
    }
}
