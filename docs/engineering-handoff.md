# Engineering handoff

Snapshot date: 2026-08-17 (Asia/Ho_Chi_Minh), after `da7b058` on `main`
plus arm64 host evidence. Local M5–M7 exit criteria are met. M8/M9 arm64
host gates are recorded; amd64 and reboot remain open.

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
- [ ] M8 — Performance and security hardening (arm64 host evidence recorded; amd64 open)
- [ ] M9 — Signed stable release and update (arm64 apply/rollback recorded; amd64 and reboot open)

## Authoritative repository state

`main` is at `da7b058` (`docs: drop disk-full from M8 gates`), with systemd
`--config` fix in `5144b39`. Arm64 evidence:
`docs/release-evidence/2026-08-17-arm64/`.

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

M8/M9 remaining native gates: **amd64** package/resource/soak/update, and
reboot survival on a host that is not sharing the live Go poller. Arm64
Ubuntu 24.04 evidence is already recorded.
