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

/// shared upgrade issue surface (WI #5 / claim shared-upgrade-issue-surface):
/// `relay upgrade` and `relay issue` expose the shared cli-std surface offline.
#[test]
fn shared_upgrade_issue_surface() {
    let issue_help = relay(&["issue", "--help"]);
    assert!(issue_help.status.success());
    let stdout = String::from_utf8(issue_help.stdout).unwrap();
    for sub in ["search", "view", "create", "comment"] {
        assert!(stdout.contains(sub), "missing {sub} in relay issue --help");
    }

    let upgrade_help = relay(&["upgrade", "--help"]);
    assert!(upgrade_help.status.success());
    let stdout = String::from_utf8(upgrade_help.stdout).unwrap();
    for flag in ["--check", "--version", "--force", "--yes"] {
        assert!(
            stdout.contains(flag),
            "missing {flag} in relay upgrade --help"
        );
    }

    let create_dry = relay(&[
        "issue",
        "create",
        "--dry-run",
        "--title",
        "relay dry-run title probe",
        "it",
        "broke",
    ]);
    assert!(create_dry.status.success());
    let stdout = String::from_utf8(create_dry.stdout).unwrap();
    assert!(
        stdout.contains("relay dry-run title probe"),
        "create dry-run output must contain title 'relay dry-run title probe'"
    );
    assert!(
        stdout.contains("app:relay"),
        "create dry-run output must contain 'app:relay'"
    );

    let comment_dry = relay(&["issue", "comment", "123", "--dry-run", "still", "broken"]);
    assert!(comment_dry.status.success());
    let stdout = String::from_utf8(comment_dry.stdout).unwrap();
    assert!(
        stdout.contains("#123"),
        "comment dry-run output must contain '#123'"
    );
    assert!(
        stdout.contains("still broken"),
        "comment dry-run output must contain 'still broken'"
    );

    for args in [
        &["issue", "view"][..],
        &["issue", "view", "abc"][..],
        &["issue", "search", "--state", "bogus"][..],
        &["issue", "comment", "0", "--dry-run"][..],
    ] {
        let out = relay(args);
        assert!(
            !out.status.success(),
            "expected non-zero exit for `relay {}`, got success",
            args.join(" ")
        );
    }
}
/// Close capability claim shared-issue-upgrade-terminal-markers (issue #7):
/// `relay issue create --dry-run` ends stdout with the chainable marker.
#[test]
fn issue_create_dry_run_ends_with_chainable_terminal_marker() {
    let out = relay(&[
        "issue",
        "create",
        "--dry-run",
        "chainable",
        "marker",
        "probe",
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let last = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("issue create dry-run prints output");
    assert_eq!(last, "next: done");
    cli_std::chainable::assert_chainable(&stdout).expect("issue create output is chainable");
    assert!(
        stdout.contains("app:relay"),
        "create dry-run output must contain 'app:relay'"
    );
}

/// Close capability claim shared-issue-upgrade-terminal-markers (issue #7):
/// `relay upgrade --check` offline ends stdout with the chainable marker.
#[cfg(not(feature = "self-update"))]
#[test]
fn upgrade_check_offline_ends_with_chainable_terminal_marker() {
    let out = relay(&["upgrade", "--check"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("latest:  unavailable"),
        "offline upgrade --check must report latest as unavailable"
    );
    let last = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("upgrade --check prints output");
    assert_eq!(last, "next: done");
    cli_std::chainable::assert_chainable(&stdout).expect("upgrade --check output is chainable");
}

/// Close capability claim shared-issue-upgrade-terminal-markers (issue #7):
/// `relay upgrade` without self-update support refuses with the stable message.
#[cfg(not(feature = "self-update"))]
#[test]
fn upgrade_without_self_update_refuses_with_stable_message() {
    let out = relay(&["upgrade"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("compiled without self-update support"),
        "stderr must say the build lacks self-update support, got: {stderr}"
    );
}

/// Negative control for the chainable oracle (issue #7): output with no
/// `next:` marker and no runnable next command must be rejected.
#[test]
fn chainable_oracle_rejects_output_without_terminal_marker() {
    let output = "repo:  faberline/relay\ntitle: no marker here\n";
    assert!(
        cli_std::chainable::assert_chainable(output).is_err(),
        "assert_chainable must reject output without a next marker"
    );
}

fn last_non_empty_line(stdout: &str) -> &str {
    stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("stdout has a non-empty line")
}

/// Close capability claim next-markers-on-write-and-backup-outputs (issue #8):
/// a write output (`spec gen --out <dir>`, a render verb with `--out <file>`)
/// ends stdout with the chainable marker, and `assert_chainable` accepts it.
#[test]
fn write_outputs_end_stdout_with_chainable_marker() {
    let dir = tempfile::tempdir().unwrap();

    let spec_gen = relay(&[
        "spec",
        "gen",
        "--lang",
        "rust",
        "--out",
        dir.path().to_str().unwrap(),
    ]);
    assert!(spec_gen.status.success());
    let stdout = String::from_utf8(spec_gen.stdout).unwrap();
    assert_eq!(last_non_empty_line(&stdout), "next: done");
    cli_std::chainable::assert_chainable(&stdout).expect("spec gen output is chainable");

    let out_file = dir.path().join("relay.yaml");
    let render = relay(&[
        "k8s",
        "instance",
        "render",
        "--profile",
        "dev",
        "--out",
        out_file.to_str().unwrap(),
    ]);
    assert!(render.status.success());
    assert!(out_file.is_file());
    let stdout = String::from_utf8(render.stdout).unwrap();
    assert_eq!(last_non_empty_line(&stdout), "next: done");
    cli_std::chainable::assert_chainable(&stdout).expect("render --out output is chainable");
}

/// Close capability claim next-markers-on-write-and-backup-outputs (issue #8):
/// the stdout-only form of a render verb prints the artifact and no `next:`
/// line, so the artifact stream stays unwrapped.
#[test]
fn stdout_only_render_prints_artifact_without_next_line() {
    let out = relay(&["k8s", "instance", "render", "--profile", "dev"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.trim().is_empty(), "render prints the artifact");
    assert!(
        stdout.lines().all(|line| !line.starts_with("next:")),
        "stdout-only render must not print a next: line"
    );
}

/// Close capability claim next-markers-on-write-and-backup-outputs (issue #8):
/// without the `backup` feature, `relay backup` refuses with the stable message.
#[cfg(not(feature = "backup"))]
#[test]
fn backup_without_feature_refuses_with_stable_message() {
    let dir = tempfile::tempdir().unwrap();
    let out = relay(&[
        "backup",
        "--url",
        "http://127.0.0.1:1",
        "--dest",
        &format!("file://{}", dir.path().display()),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("compiled without backup support"),
        "stderr must say the build lacks backup support, got: {stderr}"
    );
}
// HANDWRITE-END
