model SurveyMast "Rate-command stereo head with mechanical travel limits"
  input Real elevation;
  input Real azimuth;
  output Real pitch(start=0,fixed=true,unit="rad");
  output Real yaw(start=0,fixed=true,unit="rad");
equation
  der(pitch)=min(5*(0.955-pitch),max(-5*(0.695+pitch),0.62*elevation));
  der(yaw)=min(5*(1.13-yaw),max(-5*(1.13+yaw),0.87*azimuth));
end SurveyMast;
