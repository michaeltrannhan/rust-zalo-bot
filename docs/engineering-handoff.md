# Engineering handoff

Snapshot date: 2026-08-16 (Asia/Ho_Chi_Minh), after signed `eb21448` on
`main`. Local M5–M7 exit criteria are met. M8/M9 native host gates remain
environment-limited.

This document records the current execution state of
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`.

## Progress checklist

- [x] M0 — Decisions and measurable contracts
- [x] M1 — Installable walking skeleton
- [x] M2 — Text-command slice
- [x] M3 — Durable work depth
- [x] M4 — Receipt-to-expense slice
- [x] M5 — Real extraction and object storage (`eb21448`)
- [x] M6 — Notifications, schedules, deletion, and insights (`eb21448`)
- [x] M7 — Operator depth (`eb21448`)
- [ ] M8 — Performance and security hardening (local substitutes only)
- [ ] M9 — Signed stable release and update (local update flow only)

## Authoritative repository state

`main` is at `eb21448` (`feat: add M5-M9 storage through signed updates`).

Included locally:

- Filesystem and path-style S3 object stores; Gemini `generateContent`
  adapter; 2048-edge downscale
- Quotas, DST-correct schedules, deletion/export, insight snapshots
- Operator CLI (`status`, `jobs`, `doctor`, `ingress`, `backup`/`restore`,
  `logs`, `diagnose`), Prometheus `/metrics` (off by default), systemd
  notify/watchdog, Caddy/MinIO profiles
- SBOM generator, systemd `MemoryMax=384M` / `TasksMax=256`
- `zl-expense update preflight|apply|rollback` with Ed25519 signatures and
  schema-gated rollback

Schema versions 6–10. Default extract backend remains `fake`; insights LLM
remains off.

## Accepted residual risks

See `docs/implementation-progress.md`. Duplicate extract while `extracting`
is still possible. Path-style S3 only. Live Google smoke is not a PR gate.

## Resume commands

```bash
export TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:55439/zl_expense_test'
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
./scripts/test-package.sh
```

## Work not yet implemented

M8/M9 **native** release gates on representative Debian/Ubuntu amd64 and
arm64 hosts: resource measurements, webhook load, one-hour soak, crash
matrix, signed amd64/arm64 debs/tarballs, reboot, and host update/rollback
evidence. Local substitutes do not count as that proof.
