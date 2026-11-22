// HANDWRITE-BEGIN gap="missing-generator:unit-test:rfs-script" tracker="#48" reason="Tests of the restart/failover/soak verification wrapper."
//! Offline tests of `scripts/verify-restart-failover-soak.sh` (issue #48).
//! They run the script with bash, so they need no cluster and no soak run.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/verify-restart-failover-soak.sh"
);

#[test]
fn script_is_executable() {
    let mode = std::fs::metadata(SCRIPT)
        .expect("verify-restart-failover-soak.sh must exist")
        .permissions()
        .mode();
    assert!(
        mode & 0o100 != 0,
        "owner execute bit must be set, mode is {mode:o}"
    );
}

#[test]
fn zero_test_module_fails_the_gate() {
    let out = Command::new("bash")
        .arg(SCRIPT)
        .env("RFS_MODULES", "no_such_module_xyz")
        .env("RFS_SKIP_SOAK", "1")
        .output()
        .expect("run verify-restart-failover-soak.sh");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a module that runs 0 tests must fail the gate; stderr: {stderr}"
    );
    assert!(
        stderr.contains("0 tests ran"),
        "stderr must name the empty module; stderr: {stderr}"
    );
}

#[test]
fn module_with_tests_passes_without_soak() {
    let out = Command::new("bash")
        .arg(SCRIPT)
        .env("RFS_MODULES", "backup")
        .env("RFS_SKIP_SOAK", "1")
        .output()
        .expect("run verify-restart-failover-soak.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "backup module has tests, the gate must pass; stderr: {stderr}"
    );
    assert!(
        stdout.contains(">> backup: "),
        "stdout must report the module count; stdout: {stdout}"
    );
    assert!(
        stdout.contains(">> restart-failover-soak: passed"),
        "stdout must end with the pass line; stdout: {stdout}"
    );
}
// HANDWRITE-END
