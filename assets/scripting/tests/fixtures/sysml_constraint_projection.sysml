package ConstraintProjection {
    private import SI::m;
    private import ISQ::LengthValue;
    private import ISQMechanics::MassDensityValue;
    private import ISQMechanics::PressureValue;
    private import ISQMechanics::ModulusOfElasticityValue;

    constraint def SegmentMember {
        in start : LengthValue[3];
        in finish : LengthValue[3];
    }

    constraint def CoincidentPointTranslation {
        in movingPoint : LengthValue[3];
        in fixedPoint : LengthValue[3];
        out translation : LengthValue[3];
        movingPoint + translation == fixedPoint;
    }

    part def StrutAssembly {
        attribute upperMount : LengthValue[3] = (0.0[m], 1.0[m], 0.0[m]);
        attribute lowerMount : LengthValue[3] = (0.5[m], -1.0[m], 0.0[m]);
        attribute movingAnchor : LengthValue[3] = (0.0[m], 0.0[m], 0.0[m]);
        attribute fixedAnchor : LengthValue[3] = (1.25[m], -0.5[m], 2.0[m]);
        attribute movingAnchorSecond : LengthValue[3] = (1.0[m], 0.0[m], 0.0[m]);
        attribute fixedAnchorSecond : LengthValue[3] = (2.25[m], -0.5[m], 2.0[m]);
        attribute materialDensity : MassDensityValue;
        attribute vesselPressure : PressureValue;
        attribute elasticModulus : ModulusOfElasticityValue;
        constraint loadPath : SegmentMember {
            binding upperMountBinding bind start = upperMount;
            binding lowerMountBinding bind finish = lowerMount;
        }
        constraint alignMount : CoincidentPointTranslation {
            binding movingAnchorBinding bind movingPoint = movingAnchor;
            binding fixedAnchorBinding bind fixedPoint = fixedAnchor;
        }
        constraint alignMountSecond : CoincidentPointTranslation {
            binding movingAnchorSecondBinding bind movingPoint = movingAnchorSecond;
            binding fixedAnchorSecondBinding bind fixedPoint = fixedAnchorSecond;
        }
    }
}
