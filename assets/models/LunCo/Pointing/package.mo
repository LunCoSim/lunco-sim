within LunCo;
package Pointing "Aiming things at other things: servo axes, gimbals, antenna patterns"
  annotation(Documentation(info = "<html>
<p>Anything whose job is to HOLD A DIRECTION. A solar panel chasing the sun and a high-gain
dish holding Earth are the same problem twice — a commanded angle, a servo with lag, and a
penalty for being off target — so the servo is a component here rather than an equation
copied into each tracker.</p>

<p><code>ServoAxis</code> is the shared part: one first-order axis. <code>SunTracker</code>
(one axis, azimuth) and <code>EarthTracker</code> (two axes plus a beam pattern) are
assemblies of it. Adding a third tracker should mean instantiating this, not writing
<code>der(x) = (cmd - x)/tau</code> again.</p>

<p><code>DishPattern</code> owns the other half of pointing: what it COSTS to be off
target. A parabolic dish's main lobe is Gaussian to first order, so the link fraction and
the lock flag follow from the off-boresight angle and the beamwidth — which is itself a
function of dish diameter and wavelength, not a magic constant.</p>
</html>"));
end Pointing;
