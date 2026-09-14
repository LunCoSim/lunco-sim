---
name: luncosim-onboarding
description: Help a new LunCoSim user or AI agent choose the right skill, understand the Twin/USD/Modelica/Rhai/SysML/Python boundaries, install or connect MCP, and start a source or installed build. Use for questions like “where do I start”, “what can LunCoSim do”, “how do skills work”, or “which format/tool should I use”.
---

# LunCoSim onboarding and skill routing

Use this auto-discoverable skill as the first stop for an unfamiliar LunCoSim
request. The full cross-host router is [`../START-HERE.md`](../START-HERE.md);
read it before choosing an implementation skill.

1. State the requested outcome and inspect
   [`capability-discovery`](../capability-discovery/SKILL.md) when the feature
   is not already known.
2. Choose exactly one primary runbook from [`../README.md`](../README.md).
   Supporting skills are loaded only when that primary runbook defers to them.
3. Read the owner/source and make the smallest authored change in the format
   that owns the fact: USD for scene identity/topology/materials, Modelica for
   continuous equations, Rhai for policy/sequencing/verdicts, SysML/KerML for
   requirements and verification cases, and Rust for generic engine seams.
4. Validate with the production binary or API surface named by the runbook.
   `python3 scripts/validate_skills.py` checks this catalogue only; Python is
   an optional LunCoSim integration, not a normal runtime requirement.
5. Hand off owner, files, exact checks, runtime/visual evidence, limits, and
   branch/commit. “Not found” must name the searched scope; it is not a claim
   that the capability is impossible.

For MCP, install or run `@lunco/mcp-server` and point it at an already-running
LunCoSim API. The server exposes the packaged skill catalog and Markdown
resources, but it does not silently launch the simulator or replace the
production binary. Set `LUNCOSIM_BIN` when the simulator is installed outside
`PATH`; in a source checkout use the freshly built `target/debug/luncosim`.
