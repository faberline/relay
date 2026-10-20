//! End-to-end suites (faberline/aw docs/standards/rust-tests.md): each module
//! is one suite that talks to a relay running somewhere else, at
//! `E2E_ENDPOINT`. Every test is `#[ignore]`, so plain `cargo test` needs no
//! cluster; a suite runs with `--ignored --test-threads=1 e2e::<suite>::`.
//!
//! On Faberline's shared GKE test cluster the test Job in
//! `k8s/gke-test-job/<suite>/` runs this binary next to the deploy of
//! `k8s/overlays/gke-test` (`.github/workflows/gke-test.yml`). Cluster actions
//! are `kubectl` calls in the Job's namespace; every check is here in Rust,
//! and each step prints one `E2E_EVIDENCE {json}` line to stdout.

mod raft_failover;

use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Every suite, by module name. Each one needs a Job folder
/// `k8s/gke-test-job/<suite>/` whose filter is `e2e::<suite>::`, and an entry
/// in `.github/workflows/gke-test.yml`; [`every_suite_has_a_job_with_its_filter`]
/// checks the Job side.
const SUITES: &[&str] = &["raft_failover"];

/// Base URL of the relay under test, without a trailing slash. A missing
/// endpoint fails the test: an e2e test never passes or skips in silence.
pub(crate) fn endpoint() -> String {
    let endpoint = required_env("E2E_ENDPOINT");
    endpoint.trim_end_matches('/').to_owned()
}

/// A variable the run must set; a missing or empty one fails the test.
pub(crate) fn required_env(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
        _ => panic!("{name} is not set; the e2e suites run from the GKE test Job, which sets it"),
    }
}

/// Prints one evidence line. `detail` is merged into the line's object.
pub(crate) fn evidence(suite: &str, step: &str, detail: Value) {
    let mut line = json!({ "suite": suite, "step": step, "ok": true });
    if let (Some(line), Value::Object(detail)) = (line.as_object_mut(), detail) {
        line.extend(detail);
    }
    println!("E2E_EVIDENCE {line}");
}

/// Runs `kubectl -n $NAMESPACE <args>` and returns its stdout, or its stderr
/// as the error.
pub(crate) fn try_kubectl(args: &[&str]) -> Result<String, String> {
    let namespace = required_env("NAMESPACE");
    let output = Command::new("kubectl")
        .arg("-n")
        .arg(&namespace)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("cannot start kubectl: {err}"));
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// [`try_kubectl`] that fails the test on a kubectl error.
pub(crate) fn kubectl(args: &[&str]) -> String {
    try_kubectl(args).unwrap_or_else(|err| panic!("kubectl {}: {err}", args.join(" ")))
}

/// `kubectl get ... -o json`, parsed, or the kubectl error.
pub(crate) fn try_kubectl_json(args: &[&str]) -> Result<Value, String> {
    let mut args = args.to_vec();
    args.extend(["-o", "json"]);
    let out = try_kubectl(&args)?;
    serde_json::from_str(&out).map_err(|err| format!("kubectl {}: not JSON: {err}", args.join(" ")))
}

/// [`try_kubectl_json`] that fails the test on a kubectl error.
pub(crate) fn kubectl_json(args: &[&str]) -> Value {
    try_kubectl_json(args).unwrap_or_else(|err| panic!("{err}"))
}

/// Polls `probe` every 2 seconds until it returns `Ok`, and fails the test
/// with `what` after `timeout`. `probe` returns the last state it saw as the
/// error, for the failure message.
pub(crate) fn wait_for<T>(
    what: &str,
    timeout: Duration,
    mut probe: impl FnMut() -> Result<T, String>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        match probe() {
            Ok(value) => return value,
            Err(last) if Instant::now() >= deadline => {
                panic!("{what}: not reached within {timeout:?}; last saw: {last}")
            }
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// Blocking JSON-over-HTTP calls for the suites. relay's serve port speaks
/// HTTP/1.1 and h2c on one socket; the suites use HTTP/1.1.
pub(crate) struct Http {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl Http {
    pub(crate) fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for the e2e HTTP client");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { runtime, client }
    }

    /// `GET url`; a non-2xx status or a body that is not JSON is an error.
    pub(crate) fn get_json(&self, url: &str) -> Result<Value, String> {
        self.send(self.client.get(url), url)
    }

    /// `POST url` with a JSON body; a non-2xx status or a body that is not
    /// JSON is an error.
    pub(crate) fn post_json(&self, url: &str, body: &Value) -> Result<Value, String> {
        self.send(self.client.post(url).json(body), url)
    }

    fn send(&self, request: reqwest::RequestBuilder, url: &str) -> Result<Value, String> {
        self.runtime.block_on(async {
            let response = request
                .send()
                .await
                .map_err(|err| format!("{url}: {err}"))?;
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|err| format!("{url}: {err}"))?;
            if !status.is_success() {
                return Err(format!("{url}: HTTP {status}: {text}"));
            }
            serde_json::from_str(&text).map_err(|err| format!("{url}: not JSON: {err}: {text}"))
        })
    }
}

/// The identity the run expects, from the ConfigMap `gke-test-env`.
pub(crate) struct Expected {
    /// Version `relay --version` reports, for example `0.4.13`.
    pub(crate) version: String,
    /// First 8 hex of the commit the image was built from. The server image
    /// is built without `.git` in its context, so the binary itself reports
    /// `unknown`; the run binds the commit through the image tag instead.
    pub(crate) git_sha: String,
    /// `ghcr.io/faberline/relay@sha256:<root digest>`.
    pub(crate) image: String,
    /// `sha256:` digest the image was pushed as.
    pub(crate) root_digest: String,
    /// `sha256:` digest of the linux/amd64 image: the index's amd64 child,
    /// or the root digest itself when the push is a single manifest.
    pub(crate) runtime_digest: String,
}

impl Expected {
    pub(crate) fn from_env() -> Self {
        let version = required_env("E2E_EXPECTED_VERSION");
        let git_sha = required_env("E2E_EXPECTED_GIT_SHA");
        let image = required_env("E2E_EXPECTED_IMAGE");
        let runtime_digest = required_env("E2E_EXPECTED_RUNTIME_DIGEST");
        assert!(
            git_sha.len() == 8 && is_lower_hex(&git_sha),
            "E2E_EXPECTED_GIT_SHA must be 8 lowercase hex, got {git_sha}"
        );
        let root_digest = image
            .strip_prefix("ghcr.io/faberline/relay@")
            .filter(|digest| is_digest(digest))
            .unwrap_or_else(|| {
                panic!(
                    "E2E_EXPECTED_IMAGE must be ghcr.io/faberline/relay@sha256:<64 hex>, got {image}"
                )
            })
            .to_owned();
        assert!(
            is_digest(&runtime_digest),
            "E2E_EXPECTED_RUNTIME_DIGEST must be sha256:<64 hex>, got {runtime_digest}"
        );
        Self {
            version,
            git_sha,
            image,
            root_digest,
            runtime_digest,
        }
    }

    /// Whether a container status `imageID` binds this image: its digest is
    /// the root or the platform image.
    pub(crate) fn binds(&self, image_id: &str) -> bool {
        image_digest(image_id)
            .is_some_and(|digest| digest == self.root_digest || digest == self.runtime_digest)
    }
}

/// Checks that exactly `want` live pods match `label`, that the one container
/// of each runs the expected image, and that each runtime `imageID` binds it.
/// Returns the image IDs.
pub(crate) fn assert_pods_run(expected: &Expected, label: &str, want: u64) -> Vec<String> {
    let pods = kubectl_json(&["get", "pods", "-l", label]);
    let live: Vec<&Value> = pods["items"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|p| p["metadata"]["deletionTimestamp"].is_null())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(live.len() as u64, want, "pods with {label}");
    live.iter()
        .map(|pod| {
            let pod_name = pod["metadata"]["name"].as_str().unwrap_or_default();
            let container = only_container(&pod["spec"]);
            assert_eq!(
                container["image"],
                expected.image.as_str(),
                "pod {pod_name} image"
            );
            let image_id = pod["status"]["containerStatuses"][0]["imageID"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            assert!(
                expected.binds(&image_id),
                "pod {pod_name} runs imageID {image_id:?}, not {} or {}",
                expected.root_digest,
                expected.runtime_digest
            );
            image_id
        })
        .collect()
}

/// The single container of a pod spec.
pub(crate) fn only_container(pod_spec: &Value) -> &Value {
    match pod_spec["containers"].as_array().map(Vec::as_slice) {
        Some([container]) => container,
        _ => panic!("expected exactly one container in {pod_spec}"),
    }
}

/// The `sha256:` digest of a container status `imageID`, in any of the forms
/// a runtime reports: `<repo>@sha256:…`, `docker-pullable://<repo>@sha256:…`,
/// or `containerd://sha256:…`.
pub(crate) fn image_digest(image_id: &str) -> Option<&str> {
    let digest = match image_id.rsplit_once('@') {
        Some((_, digest)) => digest,
        None => image_id
            .rsplit_once("://")
            .map_or(image_id, |(_, rest)| rest),
    };
    is_digest(digest).then_some(digest)
}

fn is_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && is_lower_hex(hex))
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[test]
fn image_digest_reads_every_runtime_form() {
    let digest = format!("sha256:{}", "a".repeat(64));
    for image_id in [
        format!("ghcr.io/faberline/relay@{digest}"),
        format!("docker-pullable://ghcr.io/faberline/relay@{digest}"),
        format!("containerd://{digest}"),
        digest.clone(),
    ] {
        assert_eq!(image_digest(&image_id), Some(digest.as_str()), "{image_id}");
    }
    assert_eq!(image_digest("ghcr.io/faberline/relay:0.4.13"), None);
    assert_eq!(image_digest("sha256:ABC"), None);
}

/// Guard against a GKE run that passes with `running 0 tests`: every suite
/// has a Job whose filter is `e2e::<suite>::`, every Job folder belongs to a
/// suite, and the suite module has at least one ignored test that the filter
/// selects. aw's run.sh also fails a `running 0 tests` log, but this catches
/// a renamed module or a filter typo before any cluster time is spent.
#[test]
fn every_suite_has_a_job_with_its_filter() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let jobs = root.join("k8s/gke-test-job");
    let mut folders: Vec<String> = std::fs::read_dir(&jobs)
        .unwrap_or_else(|err| panic!("{}: {err}", jobs.display()))
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    folders.sort();
    let mut suites: Vec<String> = SUITES.iter().map(|s| (*s).to_owned()).collect();
    suites.sort();
    assert_eq!(folders, suites, "k8s/gke-test-job/ folders and e2e SUITES");

    let workflow = std::fs::read_to_string(root.join(".github/workflows/gke-test.yml")).unwrap();
    let here = root.join("tests/it/e2e");
    for suite in SUITES {
        let job = std::fs::read_to_string(jobs.join(suite).join("job.yaml")).unwrap();
        let filter = format!("\"e2e::{suite}::\"");
        assert!(
            job.contains(&filter),
            "k8s/gke-test-job/{suite}/job.yaml must pass the filter {filter}"
        );
        let source = std::fs::read_to_string(here.join(format!("{suite}.rs"))).unwrap();
        let ignored = source
            .matches("#[ignore = \"e2e: needs E2E_ENDPOINT\"]")
            .count();
        assert!(
            ignored > 0,
            "e2e/{suite}.rs has no #[ignore = \"e2e: needs E2E_ENDPOINT\"] test"
        );
        assert!(
            workflow.contains(&format!("\"{suite}\"")),
            ".github/workflows/gke-test.yml does not run suite {suite}"
        );
    }
}
