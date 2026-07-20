within;
package LunCo "LunCoSim's Modelica library — the math a lunar base is made of"
  annotation(Documentation(info = "<html>
<p>The equations LunCoSim simulates, as a Modelica library rather than a pile of files.</p>

<p>A package, not a folder of loose models, so that a model can USE another model:
<code>within LunCo.Electrical;</code> puts a class in a namespace its siblings can import,
which is what makes a battery and a bus composable into an EPS instead of two unrelated
files that happen to sit together. It is directory-mapped in the standard Modelica way —
one class per file, the path IS the name — so <code>LunCo.Electrical.Battery</code> lives
at <code>LunCo/Electrical/Battery.mo</code> and a USD prim names that file directly.</p>

<p>WHAT BELONGS HERE. Math. Anything that is an equation — charge integration, a motor's
draw, a panel's output. What does NOT belong: geometry, mass, colliders, joints and the
handful of contracts Avian reads by name, which are USD's; and behaviour (when to shed a
load, where to drive), which is rhai's. USD assembles the parts and holds every parameter
VALUE; this library holds what the parameters MEAN.</p>

<p>ON THE FLY. A model is bound by <code>info:sourceAsset</code>, an
<code>asset</code> — so the resolver sees it, the reference closure ships it, and editing
the <code>.mo</code> changes the law the vessel flies without touching the vessel.</p>
</html>"));
end LunCo;
