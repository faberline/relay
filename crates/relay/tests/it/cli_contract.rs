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

/// Write outputs: run `relay spec gen --lang rust --out <tempdir>` and one render verb with `--out <tempdir>`.
/// Each exits 0, its stdout's last non-empty line is exactly `next: done`, and `assert_chainable(stdout)` returns `Ok`.
#[test]
fn next_markers_on_write_outputs_spec_gen() {
    let out = tempfile::tempdir().expect("tempdir");
    let output = relay(&[
        "spec",
        "gen",
        "--lang",
        "rust",
        "--out",
        out.path().to_str().expect("valid path"),
    ]);
    assert!(
        output.status.success(),
        "relay spec gen failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("valid utf8 stdout");
    let last_non_empty = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("non-empty stdout");
    assert_eq!(last_non_empty, "next: done");
    assert!(
        cli_std::chainable::assert_chainable(stdout).is_ok(),
        "assert_chainable failed for spec gen: {stdout}"
    );
}

#[test]
fn next_markers_on_write_outputs_deploy_render() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("relay.yaml");
    let output = relay(&[
        "k8s",
        "instance",
        "render",
        "--profile",
        "dev",
        "--out",
        out.to_str().expect("valid path"),
    ]);
    assert!(
        output.status.success(),
        "relay k8s instance render failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.is_file(), "expected rendered output file to exist");
    let stdout = std::str::from_utf8(&output.stdout).expect("valid utf8 stdout");
    let last_non_empty = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("non-empty stdout");
    assert_eq!(last_non_empty, "next: done");
    assert!(
        cli_std::chainable::assert_chainable(stdout).is_ok(),
        "assert_chainable failed for k8s instance render with --out: {stdout}"
    );
}

/// Stdout-only form: the same render verb without `--out` prints the artifact and no `next:` line,
/// so the artifact stream stays unwrapped.
#[test]
fn stdout_only_render_verb_omits_next_marker() {
    let output = relay(&["k8s", "instance", "render", "--profile", "dev"]);
    assert!(
        output.status.success(),
        "relay k8s instance render stdout-only failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("valid utf8 stdout");
    assert!(!stdout.is_empty(), "expected non-empty rendered artifact");
    assert!(
        !stdout.lines().any(|l| l.trim().starts_with("next:")),
        "stdout-only render form must not contain next: marker lines: {stdout}"
    );
}
// HANDWRITE-END
