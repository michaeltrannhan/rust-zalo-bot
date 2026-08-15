# Operator install guide (Milestone 1)

Unsigned development packages and portable bundles for Debian 12, Ubuntu 22.04/24.04
on `amd64` and `arm64`. Production signed releases follow the same layout in later
milestones.

## Paths and accounts

| Path | Purpose |
| --- | --- |
| `/usr/bin/zl-expense` | Application binary |
| `/usr/share/zl-expense/migrations/` | SQL migration files |
| `/usr/share/zl-expense/config.example.toml` | Reference configuration |
| `/etc/zl-expense/config.toml` | Operator configuration (created on first install; not overwritten) |
| `/etc/zl-expense/credentials/` | Credential files (`root:zl-expense`, mode `0750`) |
| `/var/lib/zl-expense/` | State, receipt objects, update metadata |
| `/run/zl-expense/` | Runtime directory (systemd `RuntimeDirectory`) |
| `/lib/systemd/system/zl-expense.service` | Default hardened unit |

The package creates a dedicated system user `zl-expense` (group `zl-expense`) with
home `/var/lib/zl-expense`. Maintainer scripts never overwrite an existing `/etc/zl-expense/config.toml` or
files under `/etc/zl-expense/credentials/`. The file is not shipped as a package
conffile; first install copies the example only when missing.

## Clean install (Debian package)

1. Build or obtain `zl-expense_*_amd64.deb` or `zl-expense_*_arm64.deb`.
2. Install PostgreSQL (bundle default: same host) or start the development compose profile:

   ```bash
   docker compose -f compose.yaml up -d
   ```

   The compose profile pins PostgreSQL 16 and binds `127.0.0.1:5432` only.

3. Install the package:

   ```bash
   sudo dpkg -i dist/zl-expense_0.1.0~dev1_amd64.deb
   ```

4. Edit `/etc/zl-expense/config.toml` (created from the example on first install only).
   Point the database URL at loopback PostgreSQL, for example:

   ```toml
   database_url = "postgres://zl_expense:zl_expense_dev@127.0.0.1:5432/zl_expense"
   ```

5. Provision the database credential file named by `database.url_credential`
   under `/etc/zl-expense/credentials/`, owned by root and readable by the
   service group. Interactive `secret set` arrives in Milestone 7. Do not place
   secret values in `config.toml` or command arguments.

6. Apply migrations:

   ```bash
   sudo zl-expense db migrate
   ```

7. Enable and start the service:

   ```bash
   sudo systemctl enable --now zl-expense.service
   sudo systemctl status zl-expense.service
   journalctl -u zl-expense.service -n 50
   ```

8. Verify health (private listener default):

   ```bash
   curl -fsS http://127.0.0.1:8080/health/live
   curl -fsS http://127.0.0.1:8080/health/ready
   ```

## Portable tar.gz bundle

Extract the archive, then follow `INSTALL.txt` inside the bundle. The portable layout
mirrors FHS paths under a prefix you choose; production hosts should prefer the Debian
package for maintainer script safety.

## Private listener and TLS reverse proxy

Default configuration binds the HTTP listener to loopback (`127.0.0.1`). Public HTTPS
terminates in a reverse proxy (Caddy, nginx, or another TLS terminator) on the same host
or an adjacent ingress VM.

Example nginx snippet (webhook path only):

```nginx
server {
    listen 443 ssl http2;
    server_name expense.example.com;

    ssl_certificate     /etc/ssl/certs/expense.fullchain.pem;
    ssl_certificate_key /etc/ssl/private/expense.key.pem;

    location /webhooks/zalo {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        client_max_body_size 1m;
    }

    location /health/ {
        allow 127.0.0.1;
        deny all;
        proxy_pass http://127.0.0.1:8080;
    }
}
```

Firewall public ports to the proxy only; do not expose PostgreSQL or the application
loopback port on public interfaces.

## systemd unit (Milestone 1)

The packaged unit uses `Type=simple` because Milestone 1 does not yet call `sd_notify`.
Milestone 7 adds `Type=notify`, `NotifyAccess=main`, and `WatchdogSec` once the runtime
signals readiness and watchdog heartbeats.

Hardening highlights:

- `User=zl-expense`, `StateDirectory=zl-expense`, `RuntimeDirectory=zl-expense`
- `Restart=on-failure`, `RestartSec=5s`, `TimeoutStopSec=30s`
- `NoNewPrivileges=true`, `ProtectSystem=strict`, `PrivateTmp=true`, namespace and
  syscall restrictions
- Logs via journald (`StandardOutput=journal`, `SyslogIdentifier=zl-expense`)

Optional resource caps (`MemoryMax`, `TasksMax`, `CPUQuota`) are operator-tuned in
later milestones.

## Upgrade

```bash
sudo dpkg -i dist/zl-expense_<new-version>_<arch>.deb
sudo zl-expense db migrate
sudo systemctl restart zl-expense.service
```

Existing `config.toml` and credential files are preserved. New example values remain in
`/usr/share/zl-expense/config.example.toml` for manual comparison.

## Uninstall

Remove the package without deleting configuration or state:

```bash
sudo systemctl stop zl-expense.service
sudo dpkg -r zl-expense
```

`/etc/zl-expense/` and `/var/lib/zl-expense/` remain on disk.

Purge configuration, state, and the service account:

```bash
sudo dpkg --purge zl-expense
```

Purge deletes `/etc/zl-expense/` and `/var/lib/zl-expense/` and removes the
`zl-expense` user and group.

## Development PostgreSQL compose profile

From the repository root:

```bash
docker compose -f compose.yaml up -d
docker compose -f compose.yaml ps
```

Stop and remove containers (data volume persists):

```bash
docker compose -f compose.yaml down
```

Destroy the development volume explicitly:

```bash
docker compose -f compose.yaml down -v
```

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Service fails immediately | `journalctl -u zl-expense -b`; validate config with `zl-expense config validate` |
| Readiness false | `zl-expense db check`; confirm PostgreSQL reachable on loopback |
| Webhook 502 from proxy | Proxy targets loopback port; application listener not on `0.0.0.0` unless configured |
| Permission errors on credentials | Directory mode `0750`, group `zl-expense`; files readable by service user |

For redacted diagnostics, use `zl-expense diagnose` when available (Milestone 7 depth).
