model InitialFeedback
  parameter Real gain = 1.0;
  parameter Real bias = 0.0;
  input Real feedback = 0.0;
  input Real clockFeedback = 0.0;
  Real x;
  output Real sample;
  output Real solverTime;
  output Real clockSeen;
initial equation
  x = feedback;
equation
  der(x) = 0.0;
  sample = gain*x + bias;
  solverTime = time;
  clockSeen = clockFeedback;
end InitialFeedback;
