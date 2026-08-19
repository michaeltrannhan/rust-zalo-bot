//! Milestone 8 local hardening checks. Native host resource gates are env-limited.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn systemd_unit_has_hardening_and_resource_limits() {
    let unit = fs::read_to_string("deploy/systemd/zl-expense.service").expect("unit");
    for needle in [
        "Type=notify",
        "NotifyAccess=main",
        "WatchdogSec=30s",
        "NoNewPrivileges=true",
        "ProtectSystem=strict",
        "MemoryMax=384M",
        "TasksMax=256",
        "PrivateTmp=true",
        "ExecStart=/usr/bin/zl-expense --config /etc/zl-expense/config.toml run",
    ] {
        assert!(unit.contains(needle), "missing {needle}");
    }
}

#[test]
fn slot_unit_uses_per_instance_runtime_directory() {
    let unit = fs::read_to_string("deploy/systemd/zl-expense@.service").expect("template");
    for needle in [
        "RuntimeDirectory=zl-expense-%i",
        "ReadWritePaths=/var/lib/zl-expense /run/zl-expense-%i",
        "EnvironmentFile=-/etc/zl-expense/slots/%i.env",
        "TimeoutStopSec=30s",
        "MemoryMax=384M",
    ] {
        assert!(unit.contains(needle), "missing {needle}");
    }
    assert!(
        !unit.contains("RuntimeDirectory=zl-expense\n"),
        "slot units must not share /run/zl-expense"
    );
}

#[test]
fn example_config_keeps_telemetry_off_and_records_update_paths() {
    let example = fs::read_to_string("config/config.example.toml").expect("example");
    assert!(example.contains("[metrics]"));
    assert!(example.contains("enabled = false"));
    assert!(example.contains("[update]"));
    assert!(example.contains("/etc/zl-expense/update-keys"));
}

#[test]
fn metrics_source_does_not_label_account_or_merchant() {
    let source = fs::read_to_string("src/metrics/mod.rs").expect("metrics");
    assert!(!source.contains("account_id"));
    assert!(!source.contains("merchant"));
    assert!(!source.contains("job_id"));
    assert!(source.contains("KNOWN_JOB_TYPES"));
}

#[test]
fn generate_sbom_lists_this_package() {
    let dir = TempDir::new().expect("tempdir");
    let output = dir.path().join("sbom.cdx.json");
    let status = Command::new("python3")
        .args(["scripts/generate-sbom.py", output.to_str().expect("path")])
        .status()
        .expect("generate sbom");
    assert!(status.success(), "generate-sbom failed");
    let body = fs::read_to_string(&output).expect("sbom");
    assert!(body.contains("\"bomFormat\": \"CycloneDX\""));
    assert!(body.contains("zl-expense"));
    assert!(body.contains("pkg:cargo/"));
}
