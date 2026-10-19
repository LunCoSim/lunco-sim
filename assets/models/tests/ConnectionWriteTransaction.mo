model ConnectionWriteTransaction
  parameter Real initialMass(unit="kg") = 5;
  parameter Real massRate(unit="kg/s") = 120;
  parameter Real initialComX(unit="m") = 0.02;
  parameter Real speed(unit="m/s") = 0.01;
  output Real mass(unit="kg");
  output Real comX(unit="m");
  output Real sampleTime(unit="s");
equation
  mass = initialMass - massRate*time;
  comX = initialComX + speed*time;
  sampleTime = time;
end ConnectionWriteTransaction;
