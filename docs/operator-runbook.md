# Operator runbook (Milestone 7)

This runbook covers day-two operations for a self-hosted `zl-expense` deployment:
health checks, ingress failover, job recovery, backups, and redacted diagnostics.

## Network, DNS, and TLS

1. Point DNS at the TLS terminator (Caddy, nginx, or another reverse proxy).
2. Terminate HTTPS on the proxy; upstream to `127.0.0.1:8080` only.
3. Firewall public interfaces to proxy ports (443). Block PostgreSQL and the app
   loopback port from the internet.
4. Example Caddy config: `deploy/caddy/Caddyfile` (no request-body access logs).

## Credentials

- Non-secret settings live in `/etc/zl-expense/config.toml`.
- Secrets live as files under `/etc/zl-expense/credentials/` (referenced by name).
- Until the secret CLI ships, create credential files manually with mode `0640`
  and group `zl-expense`.

## Health and status

```bash
zl-expense status
zl-expense status --json
zl-expense doctor
curl -fsS http://127.0.0.1:8080/health/live
curl -fsS http://127.0.0.1:8080/health/ready
```

`status` reports migrations, ingress mode, job and outbound queue counts, and the
last inbound timestamp. It never prints secrets, message bodies, or account IDs.

## Ingress failover (audited)

```bash
zl-expense ingress status
zl-expense ingress poll      # switch to polling mode
zl-expense ingress webhook   # switch back to webhook mode
```

Each switch increments `mode_generation` in `ingress_control` for auditability.

Rollback: switch back to the previous mode with the matching command
(`ingress webhook` after an emergency `ingress poll`, or the reverse). Confirm
with `ingress status` that `mode_generation` advanced again. Coordinate the
Zalo webhook URL before returning to webhook mode (threat P-03).

## Job recovery

```bash
zl-expense jobs list --state dead
zl-expense jobs show <job-id>
zl-expense jobs retry <job-id>    # dead jobs only
zl-expense jobs cancel <job-id>   # queued or leased jobs
```

Operator views never include payload JSON, dedupe keys, or lease tokens.

## Backup and restore

```bash
zl-expense backup --output /var/backups/zl-expense.dump
zl-expense restore --input /var/backups/zl-expense.dump --yes
```

Backups use `pg_dump` custom format. Restore requires `--yes`. Database URLs are
never printed.

## Diagnostics bundle

```bash
zl-expense diagnose --output /tmp/zl-expense-diagnose
```

Writes `status.json`, `jobs-dead.json`, `config-show.json`, and `doctor.json`.
The command prints the file list before writing and rejects output containing
credential-like substrings.

## Metrics

Enable in config:

```toml
[metrics]
enabled = true
```

When enabled, `GET /metrics` on the main HTTP listener exposes Prometheus text
with allowlisted labels only (`job_type`, `error_class`, `outcome`, `ingress_source`).

## Logs

```bash
zl-expense logs
zl-expense logs --follow
zl-expense logs --since "1 hour ago"
```

Uses `journalctl -u zl-expense.service` when systemd is available.

## Incident checklist

| Symptom | Action |
| --- | --- |
| Webhook delivery failing | `ingress poll`; inspect `jobs list --state dead`; `jobs retry` |
| Outbound backlog | `status --json`; check `outbound_enabled` kill switch |
| DB unreachable | `doctor`; `db check`; verify credential file |
| After upgrade | `zl-expense update apply ... --yes`; `status --json`; `doctor` |

## Signed update

Release artifacts are verified before replacement:

```bash
zl-expense update preflight --artifact ./zl-expense --metadata ./metadata.json --signature ./metadata.sig
zl-expense update apply --artifact ./zl-expense --metadata ./metadata.json --signature ./metadata.sig --yes
zl-expense update rollback --yes   # only when previous schema range still matches
```

Trust store: `/etc/zl-expense/update-keys/`. Metadata is Ed25519-signed; the artifact
SHA-256 must match `metadata.sha256`. Automatic binary rollback is refused when the
database schema is outside the previous binary's `min_runtime_schema`/`max_runtime_schema`
(threat U-02). Restore from `pre-update.dump` in the update state directory instead.

## Privacy

See `docs/privacy-policy-template.md` for operator-facing storage, retention, and
third-party processing notes.
