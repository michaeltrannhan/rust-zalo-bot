# Optional blue/green cutover (single VM)

Default install is one `zl-expense.service` on `127.0.0.1:8080`. Most
operators should use `docs/operator-install.md`: install the Debian package
or portable bundle, migrate, and `systemctl enable --now`. A short restart
on upgrade is the supported default.

This profile is optional. Use it when a local reverse proxy already owns a
stable loopback origin and you want to start a new process, wait for
`/health/ready`, then switch the proxy before draining the old process.

## Layout

| Piece | Address / unit |
| --- | --- |
| Stable origin (Caddy, nginx, or a tunnel) | `http://127.0.0.1:8080` |
| Blue slot | `zl-expense@blue.service` on `127.0.0.1:8081` |
| Green slot | `zl-expense@green.service` on `127.0.0.1:8082` |
| Active marker | `/var/lib/zl-expense/deploy/active-slot` |

Shared: PostgreSQL, `/etc/zl-expense/config.toml`, credentials, object store.
Webhook mode may overlap two processes (inbound uniqueness + job leases).
Do not run two pollers.

Package `prerm` does not stop the service on `upgrade`, so `dpkg -i` can
replace `/usr/bin/zl-expense` while the old inode keeps serving. The new
slot execs the new binary. Migrate **before** starting that slot. Additive
schema is required for overlap; an incompatible migration needs downtime.

Example origin snippet: `deploy/caddy/origin.caddy`. Point whatever
terminates TLS (Caddy on :443, nginx, Cloudflare Tunnel) at
`http://127.0.0.1:8080` and leave that address unchanged across cutovers.

## Cutover

On the host, after the package is on disk:

```bash
sudo /usr/share/zl-expense/deploy/host-slot-deploy.sh --deb /tmp/zl-expense_*.deb
```

From a machine with SSH to that host:

```bash
export DEPLOY_HOST=your.vm.example
export DEPLOY_USER=ubuntu
export SSH_IDENTITY="$HOME/.ssh/id_ed25519"
./scripts/remote-deploy.sh dist/zl-expense_<version>_arm64.deb
```

First run starts blue on `8081`, then moves the loopback origin to Caddy on
`8080` and disables `zl-expense.service`. If
`CLOUDFLARED_CONFIG` (default `/home/ubuntu/.cloudflared/config.yml`) exists,
the script briefly retargets a loopback `service:` line so the tunnel does
not hit a closed port during that first move. Later cutovers only reload
Caddy.

If the new slot never becomes ready, the proxy is not switched.

## CI

`.github/workflows/ci.yml` runs format, clippy, tests, and package checks.
It does not install onto anyone's VM.

`.github/workflows/operator-vm-deploy.yml` is **maintainer CD** for one
provisioned host (`workflow_dispatch` only). It is not how other operators
install. Secrets, if you use that workflow: `DEPLOY_SSH_PRIVATE_KEY`,
`DEPLOY_SSH_HOST`, `DEPLOY_SSH_USER`, `DEPLOY_SSH_KNOWN_HOSTS`.
