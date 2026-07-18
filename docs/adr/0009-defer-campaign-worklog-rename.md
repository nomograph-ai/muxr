# ADR 0009: Defer the campaign -> worklog vernacular rename

- Status: Accepted
- Date: 2026-07-17
- Relates to: [ADR 0005](0005-config-discovery-open-regions.md), [ADR 0008](0008-remove-readiness-inference-recycle-sentinel.md)
- Implemented in: (deferred -- no code change in 4.0.0)

## Context

The estate vernacular drifted: what muxr calls a `campaign` (a category of work
at `campaigns/<category>/`) was decided (2026-06-05) to be renamed `worklog`, to
match the three-tier worklog/campaign/deliverable model the harness converged on.
The rename never shipped, and a stray `campaigns/muxr/` slug collision is a live
symptom. v4.0.0 is a breaking window, so folding the rename in is tempting.

## Decision

**Defer the `campaign` -> `worklog` rename out of v4.0.0.** v4 already carries a
large breaking surface (readiness removal, per-repo scoping, the config key
rename `companion` -> `viewer`, schema hardening). The vernacular rename touches
every campaign path, every operator estate repo's directory layout, the
synthesist session model, and muxr's `{campaign}` interpolation token -- a
cross-estate migration whose blast radius is orthogonal to v4's, with no
technical dependency on it. Coupling it to v4 would widen the migration and the
risk for nothing. It ships as its own change, on its own cadence, once the
harness-convergence vernacular is settled end to end.

## Consequences

- v4.0.0 keeps the `campaign` noun and the `{campaign}` interpolation token
  unchanged; no operator estate path moves as part of v4.
- The stray `campaigns/muxr/` stub is retired as housekeeping, not via the rename.
- When the rename does ship it gets its own ADR + a `muxr config migrate` arm;
  this record exists so the deferral is a decision on the trail, not an oversight.
