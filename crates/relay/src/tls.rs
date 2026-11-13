// HANDWRITE-BEGIN gap="missing-generator:logic:32cbd2eb" tracker="pending-tracker" reason="install_default_crypto_provider(): delegate to peer_tls::install_default_crypto_provider — the shared Once-guarded aws-lc-rs install. Unconditional since #1209: peer-tls (peer-mTLS material loading) links rustls into every relay build, so the former private rustls-provider feature indirection is gone. Called at the very top of main before clap parsing (keep's pattern — kube, raft-runtime peer transport and the online CLI ops all link rustls)."
//! Process-level rustls crypto provider install.
//!
//! Several relay build paths link rustls: `core/peer-tls` (peer-mTLS
//! material loading, in every build since #1209), the k8s operator (kube-rs →
//! hyper + rustls), the raft-runtime peer transport (via reqwest), and the online
//! CLI ops (`upgrade` / `issue`). rustls 0.23 refuses to pick a default crypto
//! provider when more than one is linked in the process, so any binary that
//! reaches a TLS path must install one explicitly — before the first
//! `ClientConfig` / `ServerConfig` is built — or it panics at runtime. `relay`
//! calls this once at the very top of `main`, before command parsing, so every
//! path (serve, operator, upgrade, issue, backup, spec) is covered.
//!
//! The install itself is the shared `peer_tls::install_default_crypto_provider`
//! (`Once`-guarded, ignores a provider a dependency installed first). The
//! former private `rustls-provider` feature gate is gone: peer-tls links
//! rustls unconditionally, so a conditional install would only leave a
//! panic-shaped hole in the default build.

pub mod public;

/// The prefix for core's peer-mTLS env loader: derives RELAY_PEER_TLS_CERT, RELAY_PEER_TLS_KEY, RELAY_PEER_TLS_CA and RELAY_PEER_MTLS.
pub const PEER_ENV_PREFIX: &str = "RELAY_PEER";

/// Install the aws-lc-rs rustls crypto provider as the process default, once.
///
/// Idempotent: a second call — or a provider a dependency installed first — is
/// ignored (we only need *a* default present). Delegates to the shared
/// `core/peer-tls` install (#1209).
pub fn install_default_crypto_provider() {
    peer_tls::install_default_crypto_provider();
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use peer_tls::PeerTlsConfig;

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
            .arg("tls::tests::env_probe_helper")
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
            "none" => assert!(PeerTlsConfig::from_env(PEER_ENV_PREFIX).unwrap().is_none()),
            "all" => assert!(
                PeerTlsConfig::from_env(PEER_ENV_PREFIX)
                    .unwrap()
                    .unwrap()
                    .required
            ),
            "partial" => assert!(PeerTlsConfig::from_env(PEER_ENV_PREFIX)
                .unwrap_err()
                .to_string()
                .contains("must all be set together")),
            "missing" => {
                let expected = std::env::var("RELAY_PEER_TLS_EXPECTED_PATH").unwrap();
                let err = PeerTlsConfig::from_env(PEER_ENV_PREFIX)
                    .unwrap_err()
                    .to_string();
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

    /// R3 / AC3: the PEM fixture builds both rustls configs through core peer-tls —
    /// the material is proven usable even though the
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

    /// The prefix must keep relay's `RELAY_PEER_*` env contract.
    #[test]
    fn peer_env_prefix_keeps_the_relay_contract() {
        assert_eq!(PEER_ENV_PREFIX, "RELAY_PEER");
        assert_eq!(format!("{PEER_ENV_PREFIX}_TLS_CERT"), "RELAY_PEER_TLS_CERT");
        assert_eq!(format!("{PEER_ENV_PREFIX}_TLS_KEY"), "RELAY_PEER_TLS_KEY");
        assert_eq!(format!("{PEER_ENV_PREFIX}_TLS_CA"), "RELAY_PEER_TLS_CA");
    }
}
// HANDWRITE-END
