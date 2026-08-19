# zl-expense

Privacy-conscious self-hosted Zalo expense bot (Rust walking skeleton).

## Milestone 1 scope

- Compiled CLI: `config validate/show`, `db check/migrate`, `run --roles`
- PostgreSQL migration for P0 schema (`accounts`, `provider_identities`, `inbound_events`, `ingress_control`)
- HTTP health: `GET /health/live` (process liveness) and `GET /health/ready` (config + DB + migrations)
- Supervised Tokio runtime with role selection and graceful shutdown

## Quick start (development)

```bash
export TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/zl_expense'
cargo test
cargo run -- --config config/config.example.toml config validate
cargo run -- --config config/config.example.toml db migrate
cargo run -- --config config/config.example.toml run
```

Credential references resolve from `credentials.directory` in config. For CI and local tests, set `TEST_DATABASE_URL` or `ZL_EXPENSE_DATABASE_URL` to bypass credential files.

## Configuration defaults

| Setting | Default |
| --- | --- |
| Listen address | `127.0.0.1:8080` |
| DB pool max | 5 |
| Receipt extraction concurrency | 1 |
| Outbound delivery concurrency | 4 |
| Original receipt retention | 7 days (1–30) |

Environment overrides use the `ZL_EXPENSE_*` prefix with source attribution in
`config show`. Secret values are never printed.

## Install

Production hosts install a versioned Debian package or portable bundle. See
`docs/operator-install.md`. Optional blue/green cutover:
`docs/zero-downtime-deploy.md`.

## Exit codes

See `docs/product-contracts.md` for the full taxonomy. Configuration errors exit with code `3` (`config_error`).
