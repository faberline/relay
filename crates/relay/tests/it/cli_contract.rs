// HANDWRITE-BEGIN gap="missing-generator:unit-test:relay-cli-contract" tracker="pending-tracker" reason="Prove the standard Relay CLI and shared structured-log configuration remain discoverable."
use std::process::Command;

use cli_std::chainable::assert_chainable;

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

fn last_non_empty_line(stdout: &str) -> Option<&str> {
    stdout.lines().rev().find(|line| !line.trim().is_empty())
}

/// Write outputs: `spec gen --out <dir>` and a render verb with `--out <file>`
/// end stdout with the chainable terminal marker as the last non-empty line,
/// and the whole stdout passes `assert_chainable`.
#[test]
fn write_outputs_end_stdout_with_chainable_marker() {
    let spec_dir = tempfile::tempdir().unwrap();
    let spec = relay(&[
        "spec",
        "gen",
        "--lang",
        "rust",
        "--out",
        spec_dir.path().to_str().unwrap(),
    ]);
    assert!(
        spec.status.success(),
        "relay spec gen failed: {}",
        String::from_utf8_lossy(&spec.stderr)
    );
    let spec_stdout = String::from_utf8(spec.stdout).unwrap();
    assert_eq!(last_non_empty_line(&spec_stdout), Some("next: done"));
    assert_chainable(&spec_stdout).unwrap_or_else(|e| panic!("spec gen: {e}"));

    // `--out` without an extension is a directory: the artifact is `<dir>/Dockerfile`.
    let render_dir = tempfile::tempdir().unwrap();
    let out_dir = render_dir.path().join("out");
    let render = relay(&[
        "dockerfile",
        "render",
        "--variant",
        "source",
        "--out",
        out_dir.to_str().unwrap(),
    ]);
    assert!(
        render.status.success(),
        "relay dockerfile render --out failed: {}",
        String::from_utf8_lossy(&render.stderr)
    );
    assert!(
        out_dir.join("Dockerfile").is_file(),
        "render wrote the artifact file"
    );
    let render_stdout = String::from_utf8(render.stdout).unwrap();
    assert_eq!(last_non_empty_line(&render_stdout), Some("next: done"));
    assert_chainable(&render_stdout).unwrap_or_else(|e| panic!("dockerfile render --out: {e}"));
}

/// Stdout-only form: the same render verb without `--out` prints the artifact
/// and no `next:` line, so the artifact stream stays unwrapped.
#[test]
fn stdout_only_render_prints_artifact_without_next_line() {
    let render = relay(&["dockerfile", "render", "--variant", "source"]);
    assert!(
        render.status.success(),
        "relay dockerfile render failed: {}",
        String::from_utf8_lossy(&render.stderr)
    );
    let stdout = String::from_utf8(render.stdout).unwrap();
    assert!(!stdout.trim().is_empty(), "artifact printed to stdout");
    for line in stdout.lines() {
        assert!(
            !line.starts_with("next:"),
            "stdout-only render must not print a next line, got `{line}`"
        );
    }
}
// HANDWRITE-END
