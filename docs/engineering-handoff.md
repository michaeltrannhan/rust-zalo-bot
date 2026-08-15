# Engineering handoff

Snapshot date: 2026-08-15 (Asia/Ho_Chi_Minh), updated after M3 completion and
M4 core integration.

This document records the current execution state of
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. The goal is
intentionally incomplete and must not be reported as finished until
Milestones 4 through 9 satisfy their exit gates.

## Progress checklist

- [x] M0 — Decisions and measurable contracts
- [x] M1 — Installable walking skeleton
- [x] M2 — Text-command slice
- [x] M3 — Durable work depth (exit proof in `docs/implementation-progress.md`)
- [ ] M4 — Receipt-to-expense slice
  - [x] Media intake and receipt lifecycle reviewed, corrected, signed, and
        integrated on `main`
  - [ ] Vertical image-webhook-to-confirmed-expense path (in progress via
        three bounded delegated tasks A/B/C below)
- [ ] M5 — Real extraction and object storage
- [ ] M6 — Notifications, schedules, deletion, and insights
- [ ] M7 — Operator depth
- [ ] M8 — Performance and security hardening
- [ ] M9 — Signed stable release and update

## Authoritative repository state

`main` history (all signed, signatures verified):

- `167be83 fix(work): harden delivery invariants`
- `3803922 feat(runtime): execute leased outbound jobs`
- `6fb2b8d fix(work): recheck serialization lease under advisory lock`
- `9b4d871 feat(provider): add bounded Zalo media intake`
- `8aa83c7 feat(receipt): add durable receipt lifecycle`

The full repository gate (fmt, clippy `-D warnings`, 178 tests against real
PostgreSQL, `git diff --check`) passed after each cherry-pick. The
`claim update failed` flake was root-caused to a claim race against
`idx_jobs_active_serialization_key` and fixed; 30 consecutive concurrent-test
rounds passed after the fix.

The former worker worktrees (`m3-runtime-worker`, `m4-media-provider`,
`m4-receipt-core`) are integrated; their worktrees and `agent/*` branches are
removed after integration per `.agents/README.md`.

## M4 vertical integration plan

A read-only Grok investigation produced the integration plan (recorded in the
orchestration session). Key decisions:

- Reuse `receipt.ingest` (doubles as the durable download job) and
  `receipt.extract`; reuse `outbound.deliver` for the review card. No new job
  types.
- Migration 005 adds `inbound_events.media_url` and
  `inbound_events.provider_chat_id`, and widens
  `conversation_states.pending_action_type` with `receipt_review`.
- Receipt operations invoked from ingress must run inside the open ingress
  transaction (new `*_in_transaction` APIs); the public receipt APIs begin
  their own transactions and must not be called from the decision seam.
- The runtime worker dispatches on `job_type` (new `src/runtime/jobs.rs`);
  permanent download/validation failures move submissions to
  `failed_permanent` via a new `fail_queued` API instead of stranding them.
- Delegation: Task A (ingress effect, conversation commands, in-transaction
  receipt APIs, migration 005) then Task B (HTTP image dispatch + runtime job
  handlers) then Task C (acceptance tests in `tests/m4_receipt_vertical.rs`).

## Accepted residual risks

See `docs/implementation-progress.md` (extractor duplicate work, in-memory
object store).

## Resume commands

```bash
export TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:55439/zl_expense_test'
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
git diff --check
```

## Work not yet implemented

M4 still lacks the integrated image webhook -> durable download -> stored
asset -> extraction -> review/edit/confirm/reject -> expense today/history
vertical flow (in progress). M5 through M9 are unimplemented, including real
filesystem/S3 storage, Gemini profiles, schedules/insights/deletion, operator
tooling, performance and security campaigns, SBOMs, native release-matrix
proof, signed release artifacts, and update/rollback verification.

External release gates that require representative Debian/Ubuntu amd64 and
arm64 hosts remain environment-limited; local substitutes do not count as the
release proof required by the plan.
