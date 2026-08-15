# Rust port implementation progress

This checklist is the execution ledger for
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. A milestone is complete
only when its exit proof is recorded here and the repository-level checks pass.

## Milestones

- [x] M0 — Decisions and measurable contracts
  - [x] Domain vocabulary and invariants resolved
  - [x] Clean-slate architecture ADR accepted
  - [x] Eight public test seams recorded
  - [x] Target profile, resource gates, error classes, and exit codes recorded
  - [x] Legacy capabilities have keep/improve/drop dispositions
  - [x] Threat model covers webhook, media, credentials, LLM, updates, diagnostics
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
| M0 | `m0-domain-docs` | `33c9426` | yes, full base diff | `430a4e0` | Glossary/ADR; lead moved system rules to contracts |
| M0 | `m0-contract-docs` | `06c5710` | yes, full base diff | `d3225a7` | Seams, schema decisions, legacy dispositions |
| M0 | `m0-threat-doc` | `6ad163b` | yes, full base diff | `a1411fd` | Lead reconciled dedupe and error taxonomy |

## Validation ledger

| Milestone | Command or artifact | Result | Notes |
| --- | --- | --- | --- |
| M0 | `git diff --check 66ce490..HEAD` | pass | Worker commits and lead reconciliation are whitespace-clean |
| M0 | Cross-document review | pass | Dedupe, errors, privacy defaults, and seam names agree |

## Environment-limited release gates

Native Debian/Ubuntu systemd, package upgrade/rollback, amd64 and arm64
resource measurements, and one-hour soak evidence require representative
hosts. Local substitutes may catch defects, but they do not count as the
release-gate proof defined by the product plan.
