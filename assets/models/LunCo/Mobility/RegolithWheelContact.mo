within LunCo.Mobility;
model RegolithWheelContact
  "Load-prescribed quasi-steady wheel-soil reference for a coupled native axle solve"
  input Real normalLoad(unit="N", min=0);
  input Real patchLoad(unit="N", min=0);
  input Real radius(unit="m", min=0, start=0.465);
  input Real width(unit="m", min=0, start=0.28);
  input Real longitudinalVelocity(unit="m/s");
  input Real lateralVelocity(unit="m/s");
  input Real treadVelocity(unit="m/s");
  input Real pressureGradient(unit="Pa/m", min=0, start=980000);
  input Real cohesion(unit="Pa", min=0);
  input Real frictionAngle(unit="rad");
  input Real unitWeight(unit="N/m3", min=0);
  input Real shearLength(unit="m", min=0, start=0.025);
  input Real treadFriction(min=0);
  input Real sideForceRatio(min=0)
    "Measured total side force divided by load, evaluated at the geometric yaw";
  input Real measuredSpeed(unit="m/s", min=0, start=1.07)
    "Lowest validated carriage speed";
  parameter Real renewalFloor(unit="m/s", min=1e-6)=0.1
    "Numerical regularization of contact renewal at zero motion";
  output Real longitudinalForce(unit="N");
  output Real lateralForce(unit="N");
  output Real sinkage(unit="m");
  output Real lateralDissipation(unit="W");
  output Real rollingMoment(unit="N.m") "Signed resisting moment in the forward-spin coordinate";
  output Real rollingMomentLimit(unit="N.m");
  output Real treadGainBound(unit="N.s/m") "Conservative gain of tread shear for the implicit solve";
  Real share;
  Real length(unit="m");
  Real carriageSpeed(unit="m/s");
  Real renewalSpeed(unit="m/s");
  Real shearCapacity(unit="N");
  Real treadLimit(unit="N");
  Real longitudinalSlip(unit="m/s");
  Real sideSlip(unit="m/s");
  Real mobilizedLongitudinal;
  Real mobilizedLateral;
  Real blend;
  Real rootKp;
  Real soilSide(unit="N");
  Real passiveSide(unit="N");
  Real measuredSide(unit="N");
  Real longitudinalDemand(unit="N");
  Real lateralDemand(unit="N");
  Real extraSide(unit="N");
  Real projection;
equation
  share = if noEvent(normalLoad > 0) then patchLoad/normalLoad else 0;
  sinkage = if noEvent(normalLoad > 0) then
    min(radius,(normalLoad/(pressureGradient*width*sqrt(8*radius)))^(2.0/3.0)) else 0;
  length = sqrt(8*radius*sinkage);
  carriageSpeed = sqrt(longitudinalVelocity^2 + lateralVelocity^2);
  renewalSpeed = max(renewalFloor, max(carriageSpeed, abs(treadVelocity)));
  shearCapacity = share*(cohesion*width*length + normalLoad*tan(frictionAngle));
  treadLimit = min(shearCapacity, treadFriction*patchLoad);
  longitudinalSlip = treadVelocity-longitudinalVelocity;
  sideSlip = -lateralVelocity;
  mobilizedLongitudinal = 1-exp(-abs(longitudinalSlip)*length/(shearLength*renewalSpeed));
  mobilizedLateral = 1-exp(-abs(sideSlip)*length/(shearLength*renewalSpeed));
  blend = min(1, carriageSpeed/measuredSpeed)^2*(3-2*min(1, carriageSpeed/measuredSpeed));
  rootKp = tan(0.7853981633974483 + frictionAngle/2);
  soilSide = min(treadLimit, shearCapacity*mobilizedLateral);
  passiveSide = share*length*sinkage*rootKp*
    (0.5*unitWeight*sinkage*rootKp + 2*cohesion)*mobilizedLateral;
  measuredSide = patchLoad*sideForceRatio;
  longitudinalDemand = sign(longitudinalSlip)*treadLimit*mobilizedLongitudinal;
  lateralDemand = sign(sideSlip)*((1-blend)*soilSide + blend*min(treadLimit,measuredSide));
  // The measured force is total resistance. Only its part beyond admissible tread
  // shear is assigned to soil displacement, so it is never added twice.
  extraSide = sign(sideSlip)*((1-blend)*passiveSide + blend*max(0,measuredSide-treadLimit));
  projection = if noEvent(longitudinalDemand^2+lateralDemand^2 > treadLimit^2) then
    treadLimit/sqrt(longitudinalDemand^2+lateralDemand^2) else 1;
  longitudinalForce = longitudinalDemand*projection;
  lateralForce = lateralDemand*projection + extraSide;
  lateralDissipation = -lateralForce*lateralVelocity;
  // Compaction work per unit advance for a linear pressure-sinkage law.
  // The equivalent normal-pressure moment opposes wheel roll while the
  // contact force remains the tangential shear reaction.
  rollingMomentLimit = share*radius*width*pressureGradient*sinkage^2/2;
  rollingMoment = rollingMomentLimit*treadVelocity/sqrt(treadVelocity^2+renewalFloor^2);
  treadGainBound = (3*treadLimit+shearCapacity)*length/(shearLength*renewalFloor) + rollingMomentLimit/(radius*renewalFloor);
end RegolithWheelContact;
