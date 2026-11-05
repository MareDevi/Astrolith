# E2E artifacts

Every executed end-to-end scenario writes one directory under
`artifacts/e2e/<test-id>/` (`<test-id>` = `<area>-<n>`, for example `save-04`).
The convention is the contract in
[`e2e-verification-plan.md`](../audits/e2e-verification-plan.md) §3: a scenario
without a manifest is not evidence, and the *evidence* is versioned while
machine state is not.

## Committed per case

- `manifest.json` — required; `schema`, `test_id`, `title`, `purpose`,
  `status` (`passed | failed | blocked | not_run`, never inferred), `layer`,
  `started_at`, `duration_s`, `source`, `environment`, `fixture`, `command`,
  `repro`, `expected`, `observed`, `evidence`, `metrics`, `limitations`,
  `rerun_notes`.
- `command.txt` — the exact command line(s).
- `stdout.log`, `stderr.log`, `check.log`, `perf.log` — text logs.
- `state-before.json`, `state-after.json` — L2 debug-socket dumps below 64 KiB.
- `perf.json` — metrics for performance scenarios, using the same keys as the
  Phase 0 baseline so comparisons stay mechanical.
- `notes.md` — manual steps for L3 scenarios.

## Not committed

`.gitignore` excludes machine state that cannot be reviewed as evidence:

```
artifacts/e2e/*/screenshot-*.png
artifacts/e2e/*/core.*
artifacts/e2e/*/trace-*
```

Screenshots over 200 KiB, core dumps and profiler traces are referenced from
`manifest.json` (`evidence`, `limitations`) rather than stored in the
repository.

## Rules

- A failed or blocked run keeps its logs and a one-line cause in `observed`.
- Unexecuted scenarios are committed with `"status": "not_run"`.
- Artifacts never contain a copied project tree or `HOME`: each run uses an
  isolated `XDG_CONFIG_HOME`/`HOME` inside its own case directory (plan §4).
