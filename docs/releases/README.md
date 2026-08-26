# LunCoSim releases

This directory contains committed nightly changelist snapshots. Each snapshot
covers the committed range after the preceding timestamped nightly tag; local
uncommitted work is deliberately excluded.

## Latest snapshot

- [Nightly 2026-08-12](nightly-20260812.md) — `nightly-20260812T051950Z..6bfe66d17feb294596ec400c55be1f19c7dafb2c`
- [Nightly 2026-08-08](nightly-20260808.md) — `nightly-20260806T102113Z..76df48a5b4d3ff7ec2808f4b6f450641beaf81c4`
- Previous: [Nightly 2026-08-02](nightly-20260802.md) — `nightly-20260731T115555Z..4b46c0877224960e60652023f26005c72436c2d0`

Future snapshots are generated with
`skills/nightly-changelog/scripts/generate_changelog.py`. Each generated note
keeps the GitHub Release body short: download/install guidance, an AI-agent
mission prompt, and a link to the separate changelog. The changelog file lists
all changes since the previous nightly. The nightly workflow uses the
`release-notes` format, so this change list is not embedded in the GitHub
Release page.
