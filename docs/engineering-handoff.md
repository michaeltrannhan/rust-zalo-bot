# Engineering handoff

Snapshot date: 2026-08-16 (Asia/Ho_Chi_Minh), after M4 vertical-path
completion.

This document records the current execution state of
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. The goal is
intentionally incomplete and must not be reported as finished until
Milestones 5 through 9 satisfy their exit gates.

## Progress checklist

- [x] M0 — Decisions and measurable contracts
- [x] M1 — Installable walking skeleton
- [x] M2 — Text-command slice
- [x] M3 — Durable work depth
- [x] M4 — Receipt-to-expense slice (exit proof in
      `docs/implementation-progress.md`)
- [ ] M5 — Real extraction and object storage
- [ ] M6 — Notifications, schedules, deletion, and insights
- [ ] M7 — Operator depth
- [ ] M8 — Performance and security hardening
- [ ] M9 — Signed stable release and update

## Authoritative repository state

`main` is at `b200d50`. Relevant signed history:

- `efb1536 feat(ingress): accept image receipts and review commands`
- `b0a8c64 feat(runtime): dispatch receipt jobs and review followup`
- `9f62a53 fix(runtime): reuse review card and fence extract jobs`
- `b200d50 test(m4): cover image webhook to confirmed expense`

The M4 exit path is proven by `tests/m4_receipt_vertical.rs`: image webhook
→ bounded download → stored asset → fake extract → review card →
edit/confirm/reject → today/history, against real PostgreSQL and loopback
HTTP only.

Integrated worker worktrees (`m4-vertical-core`, `m4-vertical-wiring`,
`m4-vertical-tests`) should be removed after this snapshot.

## Accepted residual risks

See `docs/implementation-progress.md`:

- Duplicate extractor work while `extracting` (idempotent persist; FakeExtractor)
- Process-local `InMemoryObjectStore` loses originals across worker restart

`expire_reviews` and `retention_sweep` exist as tested APIs. The scheduler
and maintenance roles are still idle; wiring them is M6/M7 work, not an M4
exit blocker.

## Resume commands

```bash
export TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:55439/zl_expense_test'
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
git diff --check
```

## Work not yet implemented

M5 through M9, including real filesystem/S3 storage, Gemini profiles,
schedules/insights/deletion, operator tooling, performance and security
campaigns, SBOMs, native release-matrix proof, signed release artifacts, and
update/rollback verification.

External release gates that require representative Debian/Ubuntu amd64 and
arm64 hosts remain environment-limited; local substitutes do not count as the
release proof required by the plan.
