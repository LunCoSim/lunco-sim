package ConstraintProjection {
    private import ISQ::LengthValue;

    constraint def SegmentMember {
        in start : LengthValue[3];
        in end : LengthValue[3];
    }

    constraint def CoincidentPointTranslation {
        in movingPoint : LengthValue[3];
        in fixedPoint : LengthValue[3];
    }

    part def StrutAssembly {
        attribute upperMount : LengthValue[3] = (0.0[m], 1.0[m], 0.0[m]);
        attribute lowerMount : LengthValue[3] = (0.5[m], -1.0[m], 0.0[m]);
        attribute movingAnchor : LengthValue[3] = (0.0[m], 0.0[m], 0.0[m]);
        attribute fixedAnchor : LengthValue[3] = (1.25[m], -0.5[m], 2.0[m]);
        constraint loadPath : SegmentMember {
            binding upperMountBinding bind start = upperMount;
            binding lowerMountBinding bind end = lowerMount;
        }
        constraint alignMount : CoincidentPointTranslation {
            binding movingAnchorBinding bind movingPoint = movingAnchor;
            binding fixedAnchorBinding bind fixedPoint = fixedAnchor;
        }
    }
}
