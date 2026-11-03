model InitialInheritedFeedback
  parameter Real gain = 1.0;
  parameter Real bias = 0.0;
  extends InitialInheritedFeedbackBase(g=gain, b=bias);
end InitialInheritedFeedback;

model InitialInheritedFeedbackBase
  parameter Real g = 1.0;
  parameter Real b = 0.0;
  input Real feedback = 0.0;
  input Real piloted = 0.0;
  input Real manual_axis = 0.0;
  Real x;
  output Real sample;
initial equation
  x = feedback + piloted;
equation
  der(x) = 0.0;
  sample = g*x + b;
end InitialInheritedFeedbackBase;
