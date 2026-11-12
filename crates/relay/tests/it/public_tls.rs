// HANDWRITE-BEGIN gap="missing-generator:e2e-test:public-tls" tracker="pending-tracker" reason="Public client API TLS (#136): HTTPS round trip, tokenless probes over TLS, cleartext unchanged, a partial TLS env refused at startup."
//! Public client API TLS (#136).
//!
//! The TLS tests start the shared shell's TLS serve loop with a certificate
//! for `localhost` signed by a test CA. The startup tests run the real
//! `relay serve` binary.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair};
use relay::queue::server::{router, AppState};
use relay::queue::server_config::RelayServerConfig;
use relay::queue::wire::{AckResponse, LeaseRequest, LeaseResponse};
use relay::tls::public::{PublicTlsConfig, CERT_VAR, KEY_VAR};
use serde_json::json;

/// A test CA and a server certificate for `localhost`, written to a temp dir.
struct Pki {
    _dir: tempfile::TempDir,
    ca_pem: String,
    cert: PathBuf,
    key: PathBuf,
}

fn pki() -> Pki {
    let mut ca_params = CertificateParams::new(Vec::new()).unwrap();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "Relay public test CA");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();

    let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
    params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key, &ca, &ca_key).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("tls.crt");
    let key_path = dir.path().join("tls.key");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    Pki {
        _dir: dir,
        ca_pem: ca.pem(),
        cert: cert_path,
        key: key_path,
    }
}

/// Start the public listener with TLS on an ephemeral port, the way
/// `relay serve` does when both variables are set.
async fn start_tls_server(pki: &Pki) -> SocketAddr {
    let cfg = PublicTlsConfig {
        cert: pki.cert.clone(),
        key: pki.key.clone(),
    };
    let server = Arc::new(cfg.rustls_server_config().unwrap());
    let state = AppState::new(RelayServerConfig::ephemeral(), 8 * 1024 * 1024);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(service_http::serve_tls(
        listener,
        app,
        service_http::config_source(move || Some(server.clone())),
        std::future::pending::<()>(),
    ));
    addr
}

/// A client that trusts the test CA and resolves `localhost` to `addr`.
fn tls_client(pki: &Pki, addr: SocketAddr) -> reqwest::Client {
    reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(pki.ca_pem.as_bytes()).unwrap())
        .resolve("localhost", addr)
        .build()
        .unwrap()
}

fn https(addr: SocketAddr, path: &str) -> String {
    format!("https://localhost:{}{path}", addr.port())
}

fn consume_frame(v: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap();
    let mut buf = (body.len() as u32).to_be_bytes().to_vec();
    buf.extend_from_slice(&body);
    buf
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_publish_and_consume_roundtrip() {
    let pki = pki();
    let addr = start_tls_server(&pki).await;
    let client = tls_client(&pki, addr);

    let published = client
        .post(https(addr, "/v1/s/publish"))
        .json(&json!({ "message_id": "m0", "payload": { "n": "m0" } }))
        .send()
        .await
        .unwrap();
    assert!(published.status().is_success(), "publish over TLS");

    // Consume over the same TLS connection: subscribe, read one down frame,
    // then ack it.
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let req_stream = futures::stream::unfold(req_rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<Vec<u8>, std::convert::Infallible>(chunk), rx))
    });
    req_tx
        .send(consume_frame(
            &json!({ "type": "subscribe", "prefetch": 1 }),
        ))
        .await
        .unwrap();
    let mut resp = client
        .post(https(addr, "/v1/s/consume"))
        .body(reqwest::Body::wrap_stream(req_stream))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut buf = Vec::new();
    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if buf.len() >= 4 {
                let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if buf.len() >= 4 + len {
                    let frame = buf[4..4 + len].to_vec();
                    buf.drain(..4 + len);
                    return frame;
                }
            }
            match resp.chunk().await {
                Ok(Some(chunk)) => buf.extend_from_slice(&chunk),
                Ok(None) => panic!("stream ended before the frame was complete"),
                Err(e) => panic!("stream read error: {e}"),
            }
        }
    })
    .await
    .expect("timeout reading the down frame");
    let down: serde_json::Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(down["message_id"], "m0");
    assert_eq!(
        down["payload"],
        json!({ "n": "m0" }),
        "body round-trips over TLS"
    );
    req_tx
        .send(consume_frame(&json!({
            "type": "ack",
            "lease_id": down["lease_id"],
            "epoch": down["epoch"],
        })))
        .await
        .unwrap();
    drop(req_tx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_readyz_is_tokenless() {
    let pki = pki();
    let addr = start_tls_server(&pki).await;
    let client = tls_client(&pki, addr);
    let resp = client.get(https(addr, "/readyz")).send().await.unwrap();
    assert_eq!(resp.status(), 200, "probes answer without a token over TLS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_lease_and_ack_over_tls() {
    // A worker lease and ack work over TLS too (the same router as cleartext).
    let pki = pki();
    let addr = start_tls_server(&pki).await;
    let client = tls_client(&pki, addr);
    client
        .post(https(addr, "/v1/q/publish"))
        .json(&json!({ "message_id": "m0", "payload": { "n": "m0" } }))
        .send()
        .await
        .unwrap();
    let lease: LeaseResponse = client
        .post(https(addr, "/v1/q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lease = lease.lease.expect("a lease was granted over TLS");
    assert_eq!(lease.seq, 0);
    let ack: AckResponse = client
        .post(https(addr, "/v1/q/ack"))
        .json(&json!({ "lease_id": lease.lease_id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ack.acked);
}

/// A child process killed when the guard drops, so a failed test leaves no
/// `relay serve` behind.
struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An address on loopback that nothing listens on right now.
fn free_addr() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// `relay` with no subcommand runs the server. The TLS variables are cleared,
/// then the given ones are set.
fn relay_serve(data_dir: &Path, set: &[(&str, &Path)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_relay"));
    cmd.arg("--bind")
        .arg(free_addr())
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove(CERT_VAR)
        .env_remove(KEY_VAR);
    for (name, path) in set {
        cmd.env(name, path);
    }
    cmd
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleartext_still_works_without_tls_vars() {
    let data = tempfile::tempdir().unwrap();
    let bind = free_addr();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_relay"));
    cmd.arg("--bind")
        .arg(&bind)
        .arg("--data-dir")
        .arg(data.path())
        .env_remove(CERT_VAR)
        .env_remove(KEY_VAR)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _child = Kill(cmd.spawn().expect("start relay serve"));

    let client = reqwest::Client::new();
    let url = format!("http://{bind}/readyz");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            assert_eq!(resp.status(), 200, "cleartext /readyz answers 200");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "relay serve did not answer cleartext {url}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn serve_fails_with_only_cert_set() {
    let pki = pki();
    let data = tempfile::tempdir().unwrap();
    let output = relay_serve(data.path(), &[(CERT_VAR, pki.cert.as_path())])
        .output()
        .expect("run relay serve");
    assert!(
        !output.status.success(),
        "relay serve must refuse a partial TLS config"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(KEY_VAR), "stderr names {KEY_VAR}: {stderr}");
}
// HANDWRITE-END
