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
- [x] M1 — Installable walking skeleton
  - [x] Rust 1.94 workspace, CLI, typed config, PostgreSQL migrations, and exit taxonomy
  - [x] Supervised roles, liveness/readiness endpoints, and graceful SIGTERM shutdown
  - [x] Loopback-only development compose defaults and hardened systemd service
  - [x] Debian package and portable tarball preserve operator state on removal
  - [x] Real arm64 package installed, started, health-checked, stopped, removed, and purged on Debian 12 systemd
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
| M1 | `m1-core` | `e2b4ef3` | yes, full base diff and untracked-output audit | `80adae9` | Harness reported failure only for generated `target/`; lead removed it before integration |
| M1 | `m1-packaging` | `8ac158a` | yes, full base diff | `8453904` | Debian, portable bundle, systemd, compose, and operator guide |
| M1 | lead integration | `8879b57` | yes, post-merge review | `8879b57` | Hardened config/errors/shutdown/migration tests and reconciled package docs |
| M1 | `m1-tests` | `7b800c6` | yes, full base diff | `40738ce` | Seven independent CLI/runtime acceptance checks; ordinary parallel runner also passed |

## Validation ledger

| Milestone | Command or artifact | Result | Notes |
| --- | --- | --- | --- |
| M0 | `git diff --check 66ce490..HEAD` | pass | Worker commits and lead reconciliation are whitespace-clean |
| M0 | Cross-document review | pass | Dedupe, errors, privacy defaults, and seam names agree |
| M1 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` | pass | 19 tests under the ordinary parallel runner; includes PostgreSQL, HTTP, exit, redaction, and SIGTERM cases |
| M1 | `cargo clippy --all-targets --all-features -- -D warnings` | pass | No warnings |
| M1 | `cargo fmt --check` | pass | Formatting clean |
| M1 | `./scripts/test-package.sh` | pass | Archive contents/modes, systemd syntax, Debian remove/purge semantics |
| M1 | Real Linux arm64 release build | pass | Rust 1.94 Debian build; binary and embedded migration installed from the generated package |
| M1 | Debian 12 systemd lifecycle | pass | Real package migrated, served live/ready, stopped in 552 ms, preserved state on remove, removed it on purge |
| M1 | arm64 artifact checksums | pass | deb `4a9d2809…bc2d3`; tarball `d727fb81…d3c5b` |

## Environment-limited release gates

Native Debian/Ubuntu systemd, package upgrade/rollback, amd64 and arm64
resource measurements, and one-hour soak evidence require representative
hosts. Local substitutes may catch defects, but they do not count as the
release-gate proof defined by the product plan.
