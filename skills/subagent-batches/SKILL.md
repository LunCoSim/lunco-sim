---
name: subagent-batches
description: Run fix/refactor work with parallel subagents split into non-overlapping file batches; agents never build or test — the coordinator verifies once at the end. Use for multi-finding fix sweeps (review reports, audits, migrations).
---

# Subagent batches — parallel fixes without collisions

Workflow for executing many independent code changes (e.g. findings from a review report) with parallel subagents.

## Core rules

1. **Prioritize first, run second.** Read the source reports yourself, group findings, present the batch plan to the user, get agreement on scope BEFORE launching any fix agent. Deferred scopes (e.g. multiplayer/RBAC while focused on singleplayer) get a comment-only batch, not fixes.

2. **Split work into lots with disjoint file ownership.** Each agent gets an explicit list of files it owns and the instruction "other agents own other files — do not touch anything else." No two agents in flight may share a file. Group by file/crate proximity, not by finding severity — the lot boundary is the file set.

3. **Agents never run builds or tests.** Every agent prompt includes: "do NOT run cargo, tests, or builds of any kind; do not commit." The coordinator runs ONE `cargo check -j=2` + one test pass at the very end, after all batches land (or when the user asks). This avoids N agents compiling the same workspace concurrently.

4. **Launch each batch's agents in a single message** (parallel tool calls). Batches are sequential rounds; agents within a batch are parallel.

5. **New batches must avoid files owned by still-running agents.** Track ownership; a finished agent's files are free for the next round.

## Agent prompt template

Every fix-agent prompt contains:
- Repo path + which report file(s) and sections to read for finding details.
- "Verify each cited site by reading the CURRENT code before editing" — findings go stale (line drift, refactors, already-fixed). If a file was edited by an earlier batch, say so explicitly ("line numbers may have shifted; do not disturb those changes").
- The exact item list with file:line and expected fix direction.
- Owned-file list (exclusive).
- Style rules: match surrounding code; no comments explaining the change or its provenance; smallest correct diff; don't overengineer.
- Required return format: terse list of file → what changed → anything NOT fixed and why.

## Coordinator duties between batches

- **Handoffs:** if an agent reports an item it couldn't fix because the file belongs to another lot, either (a) SendMessage the owning agent if still running, or (b) fix it yourself if tiny, or (c) queue it for the next batch. Never let it silently drop.
- **Caveat forwarding:** when a later report refutes/downgrades a finding an in-flight agent is working on, SendMessage the agent immediately with the correction rather than waiting.
- **Cross-check agent claims that remove code paths:** if an agent deletes "the only call site" of something (e.g. the only registry-removal path), verify the replacement path actually exists before moving on.
- **False-positive tolerance:** agents should report "finding is stale/wrong, nothing to change" as a valid outcome — put nuances you already know (e.g. deliberate bypass patterns) into the prompt so agents don't "fix" intentional code.

## End of run

- Single build check + single test pass over touched crates (respect project build rules, e.g. `-j=2`).
- Summarize per-lot: fixed / skipped-with-reason / deferred-with-comment.
- User commits; never commit or push from the workflow.
