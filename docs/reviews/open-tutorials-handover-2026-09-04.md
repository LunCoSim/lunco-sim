# Tutorials branch handover

> Status: Paused at Trello authentication · Date: 2026-09-04 · Worktree:
> tutorials

## Scope

Continue the Trello repair loop from the `tutorials` checkout. Select only
open, non-blue cards whose real implementation is tutorial curriculum,
authored Rhai/scenarios, tutorial tests, documentation, or tutorial assets.
Do not claim blue cards. Do not modify Rust for this workstream.

Before editing a card:

1. Read the canonical board and resolve lists and labels from the live board.
2. Read the full card and confirm its `Worktree:` line.
3. Move `Todo` to `In Progress` and keep the card's matching label attached.
4. Record the intended scope and branch in the card description.

Completed implementation goes to `Review`, never `Done`; `Acceptance → Done`
is the user's acceptance gate.

## Verified repository state

- Current checkout: `/home/rod/Documents/luncosim-workspace/tutorials`.
- Branch: `tutorials`, clean, `HEAD=7be66aa80` (`fix(ui): fence lander HUD
  exposure across scene replacement`), ahead of `origin/tutorials` by 461
  commits.
- Integration checkout: `/home/rod/Documents/luncosim-workspace/main`.
- Branch: `main`, clean, `HEAD=da9cb9c8e` (`fix(usd): scope preview
  readiness to its root`), ahead of `origin/main` by one local commit.
- The `main` commit is already present locally and must be inspected before
  pushing. Do not overwrite it or merge the whole `tutorials` branch: the
  branches contain broad unrelated history and the tutorial branch is not a
  safe wholesale merge target.
- The `usd` checkout is outside this handover's scope. Preserve any work owned
  by that checkout.

## Trello blocker and OAuth recovery

The official Trello MCP is configured, but both `trelloReadMember` and
`trelloReadBoard` currently return `Auth required`. `codex mcp list` showing
`OAuth` is not sufficient proof of a usable session.

The previous browser failure was caused by mismatched OAuth sessions: an old
Vivaldi tab contained a stale callback URL while a later listener used a
different local port. A later fresh login attempt was cancelled. No Trello
card was changed during this handover, and no active OAuth listener remains.

Recovery, using the already authenticated Vivaldi profile:

```sh
codex mcp login trello
# Open the exact one-time URL printed by this process in Vivaldi.
# Keep this process running until it reports successful login.
```

Do not reuse an old authorization tab or start a second login listener. Verify
the result with the official tools:

```text
trelloReadMember(action="get_me")
trelloReadBoard(action="get", boardId="https://trello.com/b/bDMb0zwJ/luncosim")
```

After authentication, reread the live board and lists. Previous cached card
states are not authoritative.

## Implementation and integration protocol

For each eligible card:

1. Claim it in Trello and preserve/add its correct worktree label.
2. Inspect the authoritative owner and make the smallest root-cause change.
3. Run the narrowest relevant authored Rhai/tutorial check. For runtime
   evidence use the production `target/debug/luncosim` binary with an absolute
   `--scene` and a free explicit `--api` port; do not use a custom runner or an
   old sandbox executable.
4. Run `git diff --check`, review the complete diff for stale APIs, shims,
   duplicate mechanisms, and unrelated changes, then commit the scoped work.
5. Update the card with files, checks, evidence, limitations, and the exact
   branch/commit, then move it to `Review`.
6. In the `main` checkout, verify there is no dirty work or competing merge.
   Integrate the scoped commit safely, verify `main`, and fast-forward the
   task branch to the resulting `main` tip. Push only after the integrated
   state is verified.
7. Reread Trello and continue with the next eligible card.

If Trello remains unavailable, stop before claiming or editing another card;
record the authentication blocker rather than inventing card state.
