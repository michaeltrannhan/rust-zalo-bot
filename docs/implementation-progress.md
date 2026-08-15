# Rust port implementation progress

This checklist is the execution ledger for
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. A milestone is complete
only when its exit proof is recorded here and the repository-level checks pass.

## Milestones

- [ ] M0 — Decisions and measurable contracts
  - [ ] Domain vocabulary and invariants resolved
  - [ ] Clean-slate architecture ADR accepted
  - [ ] Eight public test seams recorded
  - [ ] Target profile, resource gates, error classes, and exit codes recorded
  - [ ] Legacy capabilities have keep/improve/drop dispositions
  - [ ] Threat model covers webhook, media, credentials, LLM, updates, diagnostics
- [ ] M1 — Installable walking skeleton
- [ ] M2 — Text-command slice
- [ ] M3 — Durable work depth
- [ ] M4 — Receipt-to-expense slice
- [ ] M5 — Real extraction and object storage
- [ ] M6 — Notifications, schedules, deletion, and insights
- [ ] M7 — Operator depth
- [ ] M8 — Performance and security hardening
- [ ] M9 — Signed stable release and update

## Integration ledger

Record every isolated worker commit here after reviewing its full diff and
before or immediately after integration.

| Milestone | Worker | Commit | Reviewed | Integrated | Notes |
| --- | --- | --- | --- | --- | --- |

## Validation ledger

| Milestone | Command or artifact | Result | Notes |
| --- | --- | --- | --- |

## Environment-limited release gates

Native Debian/Ubuntu systemd, package upgrade/rollback, amd64 and arm64
resource measurements, and one-hour soak evidence require representative
hosts. Local substitutes may catch defects, but they do not count as the
release-gate proof defined by the product plan.
