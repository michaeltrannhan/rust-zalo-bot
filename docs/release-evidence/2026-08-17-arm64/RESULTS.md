# M8/M9 arm64 host evidence

Host: Ubuntu 24.04 aarch64 (`instance-20260813-1023`), 2 vCPU / 11 GiB.
Constraint: systemd `CPUQuota=100%`, `MemoryMax=384M`.
Isolation: Rust app `127.0.0.1:8088` + Postgres `127.0.0.1:55432`. Live Go bot on `:8080` / `:5432` was left running.

Tree: `da7b058`. Package: `zl-expense_0.1.0~dev1_arm64.deb` (repack after systemd `--config` fix).
SHA-256 (repack): `d6c6d2bc730121c9577e724f8889fb2f25909d58c9c83ac9fb4fe1fa0c716440`

## M8

| Gate | Result |
| --- | --- |
| systemd Type=notify, WatchdogSec=30s | observed (`Started` after listen) |
| Ready | 97.5 ms |
| Idle RSS (10 min, 60 samples) | 11 468 KiB, flat |
| Unit CPU over burst+idle+soak (~70 min) | 21.4 s ≈ 0.5% of one CPU |
| 25 rps × 20 s webhook | 500/500 HTTP 200, p95 10.4 ms |
| Burst RSS | 12 104 KiB |
| 1 h mixed soak | 3577/3577 HTTP 200; RSS 11 532 → 12 168 KiB; tasks=2 |
| SIGTERM stop | 52.3 ms |
| kill -9 | systemd restarted; ready; dead jobs=0 |
| Oversize / unauth | HTTP 413 / 401 |
| Queue after burst | returned to 0 |

Disk-full is out of scope (product decision 2026-08-17).

## M9 (arm64, no reboot)

| Step | Result |
| --- | --- |
| `dpkg -i` upgrade of repacked unit | active, listens on `:8088` |
| `update preflight` | ok, schema 10, signed metadata |
| `update apply --yes` | backup written, package_version `0.1.0-m89host` |
| `update rollback --yes` | previous binary restored; ready |
| Apply broken artifact while stopped | `health_failed`; previous ELF restored; ready |

Reboot was not performed: this VM also runs a live Go `api -poll` on `:8080` that is not this unit.

## Remaining for milestone close

- Native **amd64** package, resource, soak, and update evidence
- Reboot survival on a host that does not share the live Go process
