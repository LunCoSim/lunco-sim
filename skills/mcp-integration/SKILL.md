---
name: mcp-integration
description: Install, register, test, or troubleshoot the LunCoSim MCP server and its portable skills. Use when an agent needs MCP tools, skill discovery, host configuration, an installed npm package, or API connection troubleshooting.
---

# MCP integration: connect the host to LunCoSim and its skills

Read [`mcp/README.md`](../../mcp/README.md), [`skills/START-HERE.md`](../START-HERE.md),
and [`docs/apps/README.md`](../../docs/apps/README.md). The MCP server is a
standard stdio server usable by Claude Code, Claude Desktop, Cline, Cursor,
Windsurf, Codex, and other MCP-compatible hosts.

## Lifecycle

1. Start this agent's production `luncosim` session with an explicit free
   `--api PORT`, from the same checkout and working directory as its terminal.
   MCP connects to that API; it does not launch, rebuild, or replace the
   simulator. Verify `/api/ready` and use `DiscoverSchema` for live commands.
2. Register either the published `@lunco/mcp-server` package or the checkout's
   `.mcp.json`. Other agents may run on their own ports; do not control their
   sessions or reuse an occupied API port.
3. Call `list_skills` or read `lunco://skills` before declaring that a workflow
   is unavailable. Use `read_skill(name)` or `lunco://skills/<name>` to load
   the portable Markdown runbook, then follow its owner and evidence rules.
4. Use simulation tools for the live API and skill resources for development
   guidance. They are complementary surfaces, not duplicate implementations.

For a source checkout, set `LUNCOSIM_BIN` to the freshly built production
binary. For an installed GitHub build, set it to the installed command or
absolute path. Node/npm packaging and Python catalogue validation are tooling
choices; Python remains an optional LunCoSim integration, not a runtime
dependency.

## Acceptance

Test the package with `npm pack --dry-run` and confirm the tarball contains
`skills/*/SKILL.md`. Test the server's skill listing and read operations without
an API session; test simulation tools separately against a live `/api/ready`
endpoint. Keep host-specific registration examples in `mcp/README.md`, not in
individual skills.
