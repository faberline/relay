// HANDWRITE-BEGIN gap="missing-generator:unit-test:relay-cli-contract" tracker="pending-tracker" reason="Prove the standard Relay CLI and shared structured-log configuration remain discoverable."
use std::process::Command;

fn relay(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .output()
        .expect("run relay CLI")
}

#[test]
fn help_exposes_standard_domain_and_observability_surfaces() {
    let output = relay(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for command in ["spec", "llm", "upgrade", "issue", "k8s", "dockerfile"] {
        assert!(
            stdout.contains(command),
            "missing {command} in relay --help"
        );
    }
    for flag in [
        "--bind",
        "--data-dir",
        "--log-format",
        "--otlp-endpoint",
        "--body-limit-bytes",
    ] {
        assert!(stdout.contains(flag), "missing {flag} in relay --help");
    }
    assert!(stdout.contains("pretty"));
    assert!(stdout.contains("json"));
}

#[test]
fn offline_spec_and_three_client_languages_share_one_contract() {
    let spec = relay(&["spec", "--format", "openapi"]);
    assert!(spec.status.success());
    let stdout = String::from_utf8(spec.stdout).unwrap();
    assert!(stdout.contains("/v1/{subject}/publish"));

    for (lang, entrypoint) in [
        ("ts", "index.ts"),
        ("py", "__init__.py"),
        ("rust", "mod.rs"),
    ] {
        let out = tempfile::tempdir().unwrap();
        let generated = relay(&[
            "spec",
            "gen",
            "--lang",
            lang,
            "--out",
            out.path().to_str().unwrap(),
        ]);
        assert!(
            generated.status.success(),
            "relay spec gen {lang} failed: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        assert!(out.path().join(entrypoint).is_file());
        assert!(String::from_utf8_lossy(&generated.stdout).contains("next: done"));
    }
}

/// shared issue upgrade terminal markers (claim shared-issue-upgrade-terminal-markers):
/// prove that `relay issue` and `relay upgrade` verbs end stdout with chainable markers.
#[test]
fn shared_issue_upgrade_terminal_markers_issue_create() {
    let output = relay(&["issue", "create", "--dry-run", "test marker issue"]);
    assert!(
        output.status.success(),
        "relay issue create --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("valid utf8 stdout");
    assert!(
        stdout.contains("app:relay"),
        "expected 'app:relay' label in issue preview: {stdout}"
    );
    let last_non_empty = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("non-empty stdout");
    assert_eq!(last_non_empty, "next: done");
    assert!(
        cli_std::chainable::assert_chainable(stdout).is_ok(),
        "cli_std::chainable::assert_chainable failed for issue create"
    );
}

#[test]
#[cfg(not(feature = "self-update"))]
fn shared_issue_upgrade_terminal_markers_upgrade_check() {
    let output = relay(&["upgrade", "--check"]);
    assert!(
        output.status.success(),
        "relay upgrade --check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("valid utf8 stdout");
    assert!(
        stdout.contains("latest:  unavailable"),
        "expected 'latest:  unavailable' in upgrade check stdout: {stdout}"
    );
    let last_non_empty = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("non-empty stdout");
    assert_eq!(last_non_empty, "next: done");
    assert!(
        cli_std::chainable::assert_chainable(stdout).is_ok(),
        "cli_std::chainable::assert_chainable failed for upgrade --check"
    );
}

#[test]
#[cfg(not(feature = "self-update"))]
fn shared_issue_upgrade_terminal_markers_upgrade_no_flags_offline() {
    let output = relay(&["upgrade"]);
    assert!(
        !output.status.success(),
        "relay upgrade without self-update support should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("compiled without self-update support"),
        "missing expected error message in stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.lines().any(|line| line.trim().starts_with("next:")),
        "no next: line is required on failed upgrade: {stdout}"
    );
}
// HANDWRITE-END
