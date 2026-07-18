# muxr decision records

Architecture Decision Records: one file per decision, numbered, immutable in
spirit -- you supersede rather than rewrite. A record captures *why* a choice
was made so it is not silently re-litigated later.

## ADR or RFC? Same document, advanced by Status

- **Proposed** -- an RFC: a decision open for discussion, not yet made.
- **Accepted** -- an ADR: the decision is made. The text stays; only Status advances.
- **Superseded by [ADR NNNN]** / **Deprecated** -- history, kept for the trail.

A record starts life as an RFC (`Proposed`) and becomes an ADR (`Accepted`) in
place. No separate RFC and ADR trees, no re-filing.

## Convention

- File name: `NNNN-kebab-title.md`, zero-padded sequential (`0001`, `0002`, ...).
- Start from [`0000-template.md`](0000-template.md).
- Keep the Context neutral; state the Decision in the active voice.
- Carry RFC-depth design in the optional `Design detail` section when the
  decision has real mechanism (interfaces, wire formats, code).
- Supersede, do not rewrite: a reversed decision gets a NEW record that marks
  the old one `Superseded by [ADR NNNN]`.

## Records

| # | Title | Status |
|---|---|---|
| [0001](0001-extension-architecture.md) | A small stable core, iterated through extensions | Accepted |
| [0002](0002-readiness-gated-upgrade.md) | Readiness-gated upgrade | Superseded by [0008](0008-remove-readiness-inference-recycle-sentinel.md) |
| [0003](0003-reclaim-interrupted-sessions.md) | Reclaim interrupted sessions via a Command probe | Superseded by [0008](0008-remove-readiness-inference-recycle-sentinel.md) |
| [0004](0004-companion-pane.md) | Companion pane (auxiliary review/preview panes) | Accepted (renamed `viewer` in v4.0.0) |
| [0005](0005-config-discovery-open-regions.md) | Config discovery + open extension regions | Accepted (implemented) |
| [0006](0006-fail-loud-on-unparseable-session-files.md) | Fail loud on unparseable session files | Accepted (implemented) |
| [0007](0007-interrupt-reclaim-in-core.md) | Interrupt-reclaim in the core File probe | Superseded by [0008](0008-remove-readiness-inference-recycle-sentinel.md) |
| [0008](0008-remove-readiness-inference-recycle-sentinel.md) | Remove readiness inference; recycle via a positive sentinel handshake | Accepted |
| [0009](0009-defer-campaign-worklog-rename.md) | Defer the campaign -> worklog vernacular rename | Accepted |
