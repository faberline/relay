// HANDWRITE-BEGIN gap="missing-generator:logic:279c2a3c" tracker="pending-tracker" reason="New always-compiled thin adapter over libs/peer-tls (lumen src/tls.rs #971 pattern): ENV_PREFIX RELAY_PEER, pub PeerTlsConfig {cert,key,ca,required} + From conversions, from_env() deriving RELAY_PEER_TLS_CERT/KEY/CA + RELAY_PEER_MTLS=on|off, rustls_server_config/rustls_client_config passthroughs. Unit tests: none-set => None, all-set + on => required, partial => must-all-be-set error, mis-pointed cert path => error naming the path, PEM fixture builds both rustls configs."
//! Peer-mTLS material for the raft peer surface (WI #1209).
//!
//! ## Env contract (lumen's names with the RELAY prefix)
//!
//! - `RELAY_PEER_TLS_CERT` — path to this pod's PEM cert chain.
//! - `RELAY_PEER_TLS_KEY`  — path to its private key.
//! - `RELAY_PEER_TLS_CA`   — path to the CA bundle peers are verified against.
//! - `RELAY_PEER_MTLS=on|off` — when `on`, non-mTLS peer connections must be
//!   rejected once termination is wired.
//!
//! The PEM loading, the rustls server/client config builders, and the
//! Once-guarded crypto-provider install are generic across every service with
//! a peer/replication port and live in `core/peer-tls` (#971); this module
//! is the thin adapter that pins relay's `RELAY_PEER_TLS_*` /
//! `RELAY_PEER_MTLS` env names (lumen's `src/tls.rs` pattern).
//!
//! The serve path hands this material to `raft-runtime::PeerTransport`: peer
//! dialing uses HTTPS and Raft routes are served only on a dedicated
//! client-certificate-required listener.

use std::path::PathBuf;

use anyhow::Result;

/// The prefix passed to `peer_tls::PeerTlsConfig::from_env`: derives
/// `RELAY_PEER_TLS_CERT` / `RELAY_PEER_TLS_KEY` / `RELAY_PEER_TLS_CA` /
/// `RELAY_PEER_MTLS`, reproducing lumen's env contract with the RELAY prefix.
const ENV_PREFIX: &str = "RELAY_PEER";

/// Peer TLS material paths + the mTLS-required toggle.
#[derive(Debug, Clone)]
pub struct PeerTlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: PathBuf,
    pub required: bool,
}

impl From<peer_tls::PeerTlsConfig> for PeerTlsConfig {
    fn from(cfg: peer_tls::PeerTlsConfig) -> Self {
        Self {
            cert: cfg.cert,
            key: cfg.key,
            ca: cfg.ca,
            required: cfg.required,
        }
    }
}

impl From<PeerTlsConfig> for peer_tls::PeerTlsConfig {
    fn from(cfg: PeerTlsConfig) -> Self {
        Self {
            cert: cfg.cert,
            key: cfg.key,
            ca: cfg.ca,
            required: cfg.required,
        }
    }
}

impl PeerTlsConfig {
    /// Load from env. Returns `Ok(None)` when no TLS material is configured
    /// (plain-h2c peer transport); an error when the config is partial
    /// (all three paths must be set together) or a path does not exist
    /// (fail fast, naming the path).
    pub fn from_env() -> Result<Option<Self>> {
        Ok(peer_tls::PeerTlsConfig::from_env(ENV_PREFIX)?.map(Self::from))
    }

    /// Build a rustls server config for the peer transport (client-cert
    /// verification against the CA bundle when `required`).
    pub fn rustls_server_config(&self) -> Result<rustls::ServerConfig> {
        peer_tls::PeerTlsConfig::from(self.clone()).rustls_server_config()
    }

    /// Build a rustls client config for dialing peer transports.
    pub fn rustls_client_config(&self) -> Result<rustls::ClientConfig> {
        peer_tls::PeerTlsConfig::from(self.clone()).rustls_client_config()
    }

    pub fn peer_transport(&self) -> Result<raft_runtime::PeerTransport> {
        raft_runtime::PeerTransport::from_config(&peer_tls::PeerTlsConfig::from(self.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The peer-tls PEM fixture pair (a self-signed cert + its key), good
    // enough to prove the rustls builders compose on real material.
    const TEST_CERT: &str = r#"-----BEGIN CERTIFICATE-----
MIIC5zCCAc+gAwIBAgIJAPl6HZTX5LElMA0GCSqGSIb3DQEBCwUAMBUxEzARBgNV
BAMMCmx1bWVuLXBlZXIwHhcNMjYwNjE4MTQwODA4WhcNMjYwNzE4MTQwODA4WjAV
MRMwEQYDVQQDDApsdW1lbi1wZWVyMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIB
CgKCAQEAuwEAFs8xYsY9GDDbefwoV4FMiD9f49hs7iLVijVhUM7z5W0Xj9mXFCFS
Sn/DNb/bF9UtUoGJ0cdpjlevd6BjaXbjm2gMIDod1yKBZ2BXwT/elwRzjEIcTgR5
+GTu355VsWqugBYob8cYn2kGAMvVFUZeRBbC1IO02xbp9zABNaBHOWVdRTXODxiU
jbtB4gioNJOG1A71sto61lMmLMp4IL02k+BbuwekhCkkRGGNuqMHVAehkJwTmmxF
aPHK3LMifWgUXn51JWEhU2OiWe3Ja8/XQU5LZDvbc3vmMaJSuIMheOIkM5AXHyo4
LX62YgtuUpouYYOHkOqNRWRQfLrvywIDAQABozowODAVBgNVHREEDjAMggpsdW1l
bi1wZWVyMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgKkMA0GCSqGSIb3
DQEBCwUAA4IBAQAavsvsmN/zKL0TVx7FLEnDRbD6L4KNg3ndPrZDKncl0Df1W5kl
4jZTujiZ2CqH7CQakra3kV51EIUuKSbc0kQBsvsCIw0Fxb/JUmsui/z9uCqrqhrT
ODlcV6pETppce5JozMAZCKUyx9460/+flP7VTqHnLt1oMrM/mmaKeZ0ImSBnx8xF
0JpJN0HyX+vlbrT/9J3xxe53v7glRPZIgBlOT1eTaroXjIk6ZzOBS8bCBpNYVec5
wN93qI3ZQWwNUMB3TXJ7IBpgIrtD+z/ZhliDnk6NOLqKPXJrch0cVwlljT0Uu+DP
Qd9/aITxkqX7P0phj2cYmALL/aBJJaWRuAfw
-----END CERTIFICATE-----
"#;

    const TEST_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC7AQAWzzFixj0Y
MNt5/ChXgUyIP1/j2GzuItWKNWFQzvPlbReP2ZcUIVJKf8M1v9sX1S1SgYnRx2mO
V693oGNpduObaAwgOh3XIoFnYFfBP96XBHOMQhxOBHn4ZO7fnlWxaq6AFihvxxif
aQYAy9UVRl5EFsLUg7TbFun3MAE1oEc5ZV1FNc4PGJSNu0HiCKg0k4bUDvWy2jrW
UyYsynggvTaT4Fu7B6SEKSREYY26owdUB6GQnBOabEVo8crcsyJ9aBRefnUlYSFT
Y6JZ7clrz9dBTktkO9tze+YxolK4gyF44iQzkBcfKjgtfrZiC25Smi5hg4eQ6o1F
ZFB8uu/LAgMBAAECggEAF23pp/HvmxOBRg2hAeiQ2V3Oy+c8yVwtUay1mmpTtf8n
2Z/Qaup1HkWKfOEDATH3bkX8NrEaJllYpUfhKRjEO8t0et0PX95ILNMa6WvNst2g
ssURAQqrZy7yZSeoMgYxcFgQYuXjzRVhxV8wLFtdaBv35YoAgQW7XBPD3n96N1CV
nRpk/tPeIaqmGnC6xhtrd9zaRy1qZ3aX5Np27ZrwsMghmJyLNI6OfsS0FRK3dKQx
dkx5L5iMD0wmqC5FsR4nc+pkFsSgdv2uxtS95JDX2jOHuJj4qm5moh0Z6eXQ8lCD
Nhr+JN1TQXHVAL696tQPnQNJtdnQYshNpsC+R2Sg+QKBgQDjsyACww2OLLIO/QBJ
rzbuAgx0n4cRR7mSCZhrgO3xX+sKU1yGPNRtwj0R/dQsVClld0KB2Oa9brR2dzcE
QWSLGcRhAmpjgmYLFn6T2Odbyb5YfTMVF6ka53w4CELKQPm7cm5QsXimTqSYPwZb
Jth2e7bkEdVemzDS8C33WYio3QKBgQDSPweWZNLC+EtHlNHg4fguww6wjkUPcoxG
C8prGovcSEMuIXUnrJmRkWdKTxHud9ofvhfauB86Daf4tkaklGPuJ5CepxnMXyos
I8fSEnIyTPD6sYC37GNUMDhMU3iyxV+CsH077TwSGpjw4cntf8pqYklP8zjctjnq
wAPG6O2cxwKBgQCTEQnW3tatgo7LAXwjG2k2FtqmpLbfYV0pRstMfCyzHwm3VJpJ
FZb7AV7idPiKXR2TrJCnP0nhBlTGwz8kn3vqIA1nvuCqPvnbpX7BzXG5Jjer/cl1
kR+nAeaIZkWFTqw99q3rroTHnbnPn71iOFfNRyCcdCxE+6VwSLLXtNuAfQKBgQDA
05QW6FOxA96vQRuY4EcqRDXV0jYeq9VhbPDyeD9sAk6zIXZ8s72JF82fBpQQnZXN
ZSAltpbVPK8g2bRCv+JDC8CE8gckPOfF4e8jiU15Or4NfvzqMwEKtsr7ndbmR0WI
7Gt/qd5dUE2TJ9J2Y6z3Ezvf+tfc/bhyyDbumLVNAwKBgHj74ZKxCKE21mv1azYk
EF1sOEisJVtdtSq2PZN7hiGgvaMTSfKegRgM+12lGDvabf93LSoYX1pEHY7qIs2f
pky/zqjmfLFtvyP+vQvAL3F+5B/1XpFj2dRnAOJaWpq62Ebe2L9k4ff7EYNTL7oq
LkjT2UdpFBDZGWHwqDRhXX8k
-----END PRIVATE KEY-----
"#;

    const TEST_KEY_2: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDKu663bq8H7tFK
KSxS+hhdAv2iri2VG3ZD1IZLk/0ULACRkVkk9tKjQ9/v3PyX29MS81rdhO2/aXoa
osV5oEp12cI3PenafvOHWHgSY9PSQ77Pk7xDTmhOuGAgxdImY7etvGc+lASTMbPj
bkTjzx2rbTkTsm5qe+9BipBwrXfMEtepurRwrvxgPbE1Vf+CcWBsKuSqmBDWFp4r
gF72CqFPj/B3qyeXEcHi1DJSqOTUfSZ6x2fYbzYaGSfQLnoQCVQxwtF60Od9L0kP
h1sNLQdgVJwN+XEk933opewo72IXWcm85+lW30l/V4FnjeQv8zhwij8s9oxrEMBE
KCnJC6SzAgMBAAECggEAJQAWBiJsTX1XszpKfaZpdehK1DsH1LPbZMvP6tFKmjjh
ra+iXm2xydxkbYmejDSxlD7X8aAGKC4e++eDNQx9Kn8gxCZHnPoYhjYuS+TuTuzQ
UWA8NuiDfwKFgNW54gnHBy1ghnpLT3X9x99jMVSmrJfVl/Oz1TljVEnuzLHMMHFY
ppy6XC8ZAJfdTipSsEFXgIXfKZfwLKZ3p1zJTEnMnO2th3jwt5uuGVK8cOrThPoC
063f9WJv66raWwVvkW+NRwwjpmFL2RavufFr9qGXl8Ozg+jM2qeX1JWjWlUsRcP4
8wn2kOaVvUrjx2VOn10QoZsoDTj/3KWL4VsO5OWOQQKBgQDwQ/XXicmjeVosBL0l
lXuQx1Dd69MkGENdwHmVU409d4yns3sDW3M4sOB0D3XvJJZDdE2gFiyYp6yIp4+f
5vD6AAP21/fUUEuonl3rECJzsO1+sGeiaQLt1hAlg82mqqR69V8A4gx3EJgVS1Zl
YplHWJfTqGceM0Tg2NUycTPhUwKBgQDYAn6ImQs0exXRudcjBok30JHczpV1R4SG
eS+RQgZe4lgdknWGkUPhKTOtvzOioHAKb0VJaXj4ytK2iIgGKulsApYGGCMPUIjy
VmI4qSxCn/GQZJq3Wj3V/fbL7qOy7dOezfqb0MRh/APIF/+iJTeGqfvCTGXivi0o
tHAMIFXjIQKBgQDAGaZDKeC1Or6U+Uj99tGwBRysAvRzJI8gM1dCxGXmImKsZv1Q
xtqOFiWvxfjGNtuJnUC/YVtQxqdVL5vO/ofnawHAaBa3g2eqrsxquCDDnU+V/pct
rIZyALgyNnrD6iXORccq3pGW1Qaz5+Nw9m+VMslZqoigBd/pYpS7WvVfDwKBgA3+
q2ACOl2eE93QooWcaMt68dhSchB1H7ruowxQMMV4+KiB0qdzReiSmMyK9Ld2mYuz
hdOpx7KmwVweIGH16cWbrUiby7m0SAsbWaW+ICX44teuDPAQIq14lMLFpUl0ax+A
zNnHftGyWDRgialhq+VHog4GkRtNovNeLVw7xBTBAoGAPOwRKi49i+lKTukBQrzD
O0vtnyMxsD7qWkBf7ixh5wfEq7taAaOOCuj/u4CUjSeAfxBhqtu3odIewcJO7UQf
AS+BkvI1DrHg7OEca6RPI+zaSHmmPMonMzeeqWs0NSBMnalXIJ0THdEzDeoUfqqT
hL1jkBLctkVegPUy9dOXVF0=
-----END PRIVATE KEY-----
"#;

    const ENV_KEYS: [&str; 4] = [
        "RELAY_PEER_TLS_CERT",
        "RELAY_PEER_TLS_KEY",
        "RELAY_PEER_TLS_CA",
        "RELAY_PEER_MTLS",
    ];

    /// Exercise the real process-env loader in an isolated child test process.
    /// This keeps the parallel parent test process free from global env mutation.
    fn run_env_case(scenario: &str, vars: Vec<(&'static str, std::ffi::OsString)>) {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("peer_tls::tests::env_probe_helper")
            .arg("--ignored")
            .env("RELAY_PEER_TLS_TEST_SCENARIO", scenario);
        for key in ENV_KEYS {
            command.env_remove(key);
        }
        command.envs(vars);
        let output = command.output().expect("spawn isolated env probe");
        assert!(
            output.status.success(),
            "isolated env probe {scenario} failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "helper runs only as an isolated child of the peer TLS env tests"]
    fn env_probe_helper() {
        let Ok(scenario) = std::env::var("RELAY_PEER_TLS_TEST_SCENARIO") else {
            return;
        };
        match scenario.as_str() {
            "none" => assert!(PeerTlsConfig::from_env().unwrap().is_none()),
            "all" => assert!(PeerTlsConfig::from_env().unwrap().unwrap().required),
            "partial" => assert!(PeerTlsConfig::from_env()
                .unwrap_err()
                .to_string()
                .contains("must all be set together")),
            "missing" => {
                let expected = std::env::var("RELAY_PEER_TLS_EXPECTED_PATH").unwrap();
                let err = PeerTlsConfig::from_env().unwrap_err().to_string();
                assert!(!err.is_empty(), "error message must be non-empty");
                assert!(
                    !err.contains("PRIVATE KEY"),
                    "error message must be credential-free"
                );
                assert!(err.contains(&expected));
            }
            other => panic!("unknown peer TLS env scenario {other}"),
        }
    }

    fn write_tls_fixture(name: &str) -> PeerTlsConfig {
        let dir =
            std::env::temp_dir().join(format!("relay-peer-tls-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cert.pem"), TEST_CERT).unwrap();
        std::fs::write(dir.join("key.pem"), TEST_KEY).unwrap();
        std::fs::write(dir.join("ca.pem"), TEST_CERT).unwrap();
        PeerTlsConfig {
            cert: dir.join("cert.pem"),
            key: dir.join("key.pem"),
            ca: dir.join("ca.pem"),
            required: true,
        }
    }

    /// R3: nothing set => `Ok(None)` (plain h2c peers, today's default).
    #[test]
    fn from_env_returns_none_when_nothing_set() {
        run_env_case("none", Vec::new());
    }

    /// R3 / AC3: all three paths + `RELAY_PEER_MTLS=on` load with
    /// `required = true`.
    #[test]
    fn from_env_loads_when_all_set() {
        let dir = std::env::temp_dir().join(format!("relay-peer-tls-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["cert.pem", "key.pem", "ca.pem"] {
            std::fs::write(dir.join(name), b"DUMMY").unwrap();
        }
        run_env_case(
            "all",
            vec![
                ("RELAY_PEER_TLS_CERT", dir.join("cert.pem").into_os_string()),
                ("RELAY_PEER_TLS_KEY", dir.join("key.pem").into_os_string()),
                ("RELAY_PEER_TLS_CA", dir.join("ca.pem").into_os_string()),
                ("RELAY_PEER_MTLS", "on".into()),
            ],
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// R3: partial config is a startup error naming the var family — never a
    /// silent fallback to plaintext.
    #[test]
    fn from_env_errors_on_partial_config() {
        run_env_case(
            "partial",
            vec![("RELAY_PEER_TLS_CERT", "/tmp/dummy-cert".into())],
        );
    }

    /// R3 / AC3: a mis-pointed cert path fails fast with an error naming the
    /// missing path.
    #[test]
    fn mis_pointed_cert_path_fails_fast_naming_the_path() {
        let dir = std::env::temp_dir().join(format!("relay-peer-tls-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("key.pem"), b"DUMMY").unwrap();
        std::fs::write(dir.join("ca.pem"), b"DUMMY").unwrap();
        run_env_case(
            "missing",
            vec![
                (
                    "RELAY_PEER_TLS_CERT",
                    dir.join("missing-cert.pem").into_os_string(),
                ),
                ("RELAY_PEER_TLS_KEY", dir.join("key.pem").into_os_string()),
                ("RELAY_PEER_TLS_CA", dir.join("ca.pem").into_os_string()),
                ("RELAY_PEER_MTLS", "on".into()),
                ("RELAY_PEER_TLS_EXPECTED_PATH", "missing-cert.pem".into()),
            ],
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// R3 / AC3: the PEM fixture builds both rustls configs through the
    /// adapter passthroughs — the material is proven usable even though the
    /// raft-runtime/h2c seam cannot terminate mTLS yet (filed gap).
    #[test]
    fn builds_rustls_peer_configs_from_pem_material() {
        let cfg = write_tls_fixture("builder");
        cfg.rustls_server_config()
            .expect("server config should build");
        cfg.rustls_client_config()
            .expect("client config should build");
        std::fs::remove_dir_all(cfg.cert.parent().unwrap()).ok();
    }

    fn assert_credential_free_err<T: std::fmt::Debug>(res: Result<T>) {
        let err = res.expect_err("expected error");
        let msg = format!("{err:#}");
        assert!(!msg.is_empty(), "error message must be non-empty");
        assert!(
            !msg.contains("PRIVATE KEY"),
            "error message must not contain PRIVATE KEY"
        );
        assert!(
            !msg.contains(TEST_KEY.trim()),
            "error message must not contain private key material"
        );
    }

    #[test]
    fn mis_pointed_key_path_fails_fast_naming_the_path() {
        let dir =
            std::env::temp_dir().join(format!("relay-peer-tls-miss-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cert.pem"), b"DUMMY").unwrap();
        std::fs::write(dir.join("ca.pem"), b"DUMMY").unwrap();
        run_env_case(
            "missing",
            vec![
                ("RELAY_PEER_TLS_CERT", dir.join("cert.pem").into_os_string()),
                (
                    "RELAY_PEER_TLS_KEY",
                    dir.join("missing-key.pem").into_os_string(),
                ),
                ("RELAY_PEER_TLS_CA", dir.join("ca.pem").into_os_string()),
                ("RELAY_PEER_MTLS", "on".into()),
                ("RELAY_PEER_TLS_EXPECTED_PATH", "missing-key.pem".into()),
            ],
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mis_pointed_ca_path_fails_fast_naming_the_path() {
        let dir =
            std::env::temp_dir().join(format!("relay-peer-tls-miss-ca-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cert.pem"), b"DUMMY").unwrap();
        std::fs::write(dir.join("key.pem"), b"DUMMY").unwrap();
        run_env_case(
            "missing",
            vec![
                ("RELAY_PEER_TLS_CERT", dir.join("cert.pem").into_os_string()),
                ("RELAY_PEER_TLS_KEY", dir.join("key.pem").into_os_string()),
                (
                    "RELAY_PEER_TLS_CA",
                    dir.join("missing-ca.pem").into_os_string(),
                ),
                ("RELAY_PEER_MTLS", "on".into()),
                ("RELAY_PEER_TLS_EXPECTED_PATH", "missing-ca.pem".into()),
            ],
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_pem_cert_file_fails_rustls_configs() {
        let cfg = write_tls_fixture("bad-cert");
        std::fs::write(&cfg.cert, b"NOT_A_PEM_CERT").unwrap();
        assert_credential_free_err(cfg.rustls_server_config());
        assert_credential_free_err(cfg.rustls_client_config());
        std::fs::remove_dir_all(cfg.cert.parent().unwrap()).ok();
    }

    #[test]
    fn non_pem_key_file_fails_rustls_configs() {
        let cfg = write_tls_fixture("bad-key");
        std::fs::write(&cfg.key, b"NOT_A_PEM_KEY").unwrap();
        assert_credential_free_err(cfg.rustls_server_config());
        assert_credential_free_err(cfg.rustls_client_config());
        std::fs::remove_dir_all(cfg.cert.parent().unwrap()).ok();
    }

    #[test]
    fn non_pem_ca_file_fails_rustls_configs() {
        let cfg = write_tls_fixture("bad-ca");
        std::fs::write(&cfg.ca, b"NOT_A_PEM_CA").unwrap();
        assert_credential_free_err(cfg.rustls_server_config());
        assert_credential_free_err(cfg.rustls_client_config());
        std::fs::remove_dir_all(cfg.cert.parent().unwrap()).ok();
    }

    #[test]
    fn mismatched_key_returns_err_from_rustls_server_config() {
        let cfg = write_tls_fixture("mismatched-key");
        std::fs::write(&cfg.key, TEST_KEY_2).unwrap();
        assert_credential_free_err(cfg.rustls_server_config());
        std::fs::remove_dir_all(cfg.cert.parent().unwrap()).ok();
    }

    #[test]
    fn empty_ca_file_returns_err() {
        let cfg = write_tls_fixture("empty-ca");
        std::fs::write(&cfg.ca, b"").unwrap();
        assert_credential_free_err(cfg.rustls_server_config());
        assert_credential_free_err(cfg.rustls_client_config());
        std::fs::remove_dir_all(cfg.cert.parent().unwrap()).ok();
    }
}
// HANDWRITE-END
