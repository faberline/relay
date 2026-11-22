// HANDWRITE-BEGIN gap="missing-generator:unit-test:ce-script" tracker="#50" reason="Tests of the local competitor efficiency verification wrapper."
//! Offline tests of `scripts/verify-competitor-efficiency.sh` (issue #50).
//! They run the script with bash and `CE_SKIP_MEASURED=1`, so they need no
//! release build of the measured gate, no cluster and no network.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/verify-competitor-efficiency.sh"
);

#[test]
fn script_is_executable() {
    let mode = std::fs::metadata(SCRIPT)
        .expect("verify-competitor-efficiency.sh must exist")
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
        .env("CE_MODULES", "no_such_module_xyz")
        .env("CE_SKIP_MEASURED", "1")
        .output()
        .expect("run verify-competitor-efficiency.sh");
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
fn empty_module_list_fails_the_gate() {
    let out = Command::new("bash")
        .arg(SCRIPT)
        .env("CE_MODULES", "")
        .env("CE_SKIP_MEASURED", "1")
        .output()
        .expect("run verify-competitor-efficiency.sh");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "an empty module list must fail the gate; stderr: {stderr}"
    );
    assert!(
        stderr.contains("CE_MODULES is empty"),
        "stderr must name the empty list; stderr: {stderr}"
    );
}

#[test]
fn module_with_tests_passes_without_measured_gate() {
    let out = Command::new("bash")
        .arg(SCRIPT)
        .env("CE_MODULES", "perf_gate")
        .env("CE_SKIP_MEASURED", "1")
        .output()
        .expect("run verify-competitor-efficiency.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "perf_gate module has tests, the gate must pass; stderr: {stderr}"
    );
    assert!(
        stdout.contains(">> perf_gate: 5 passed"),
        "stdout must report the module count; stdout: {stdout}"
    );
    assert!(
        stdout.contains(">> competitor-efficiency: passed"),
        "stdout must end with the pass line; stdout: {stdout}"
    );
}
// HANDWRITE-END
