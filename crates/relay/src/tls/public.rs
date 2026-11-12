// HANDWRITE-BEGIN gap="missing-generator:logic:public-tls" tracker="pending-tracker" reason="TLS for the public client API listener (#136): env-driven PEM paths, ALPN h2 and http/1.1."
//! TLS for the public client API listener (#136).
//!
//! `RELAY_TLS_CERT` and `RELAY_TLS_KEY` (PEM file paths) turn TLS on for the
//! client listener. Both unset keeps cleartext. Exactly one set is a startup
//! error.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

/// PEM certificate chain path for the public listener.
pub const CERT_VAR: &str = "RELAY_TLS_CERT";
/// PEM private key path for the public listener.
pub const KEY_VAR: &str = "RELAY_TLS_KEY";

/// The public listener's TLS material, as file paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicTlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl PublicTlsConfig {
    /// Load from the environment. Both variables unset gives `Ok(None)`. Both
    /// set gives `Ok(Some(..))`. Exactly one set, or a path that does not
    /// exist, is an error that names the variable or the path.
    pub fn from_env() -> Result<Option<Self>> {
        Self::from_vars(std::env::var_os(CERT_VAR), std::env::var_os(KEY_VAR))
    }

    /// The logic of `from_env`, on the two raw values, so tests need not set
    /// process environment variables.
    fn from_vars(cert: Option<OsString>, key: Option<OsString>) -> Result<Option<Self>> {
        match (cert, key) {
            (None, None) => Ok(None),
            (Some(cert), Some(key)) => {
                let cfg = Self {
                    cert: PathBuf::from(cert),
                    key: PathBuf::from(key),
                };
                for path in [&cfg.cert, &cfg.key] {
                    if !path.is_file() {
                        bail!("TLS file {} does not exist", path.display());
                    }
                }
                Ok(Some(cfg))
            }
            (Some(_), None) => bail!("{CERT_VAR} is set but {KEY_VAR} is not: set both or neither"),
            (None, Some(_)) => bail!("{KEY_VAR} is set but {CERT_VAR} is not: set both or neither"),
        }
    }

    /// Build a rustls server config for the public listener. ALPN offers `h2`
    /// and `http/1.1`. No client certificate is requested.
    pub fn rustls_server_config(&self) -> Result<rustls::ServerConfig> {
        super::install_default_crypto_provider();
        let certs = {
            let mut reader = std::io::BufReader::new(
                std::fs::File::open(&self.cert)
                    .with_context(|| format!("open {}", self.cert.display()))?,
            );
            rustls_pemfile::certs(&mut reader)
                .collect::<std::result::Result<Vec<_>, _>>()
                .with_context(|| format!("read certificates from {}", self.cert.display()))?
        };
        if certs.is_empty() {
            bail!("no certificate found in {}", self.cert.display());
        }
        let key = {
            let mut reader = std::io::BufReader::new(
                std::fs::File::open(&self.key)
                    .with_context(|| format!("open {}", self.key.display()))?,
            );
            rustls_pemfile::private_key(&mut reader)
                .with_context(|| format!("read private key from {}", self.key.display()))?
                .with_context(|| format!("no private key found in {}", self.key.display()))?
        };
        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("build TLS server config from the certificate and key")?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-signed certificate and key for `localhost`, written to `dir`.
    fn write_fixture(dir: &std::path::Path) -> PublicTlsConfig {
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "localhost");
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key.serialize_pem()).unwrap();
        PublicTlsConfig {
            cert: cert_path,
            key: key_path,
        }
    }

    #[test]
    fn from_env_none_when_unset() {
        // The test process does not set the variables; this only reads them.
        if std::env::var_os(CERT_VAR).is_none() && std::env::var_os(KEY_VAR).is_none() {
            assert_eq!(PublicTlsConfig::from_env().unwrap(), None);
        }
    }

    #[test]
    fn from_env_some_when_both_set() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_fixture(dir.path());
        let got =
            PublicTlsConfig::from_vars(Some(cfg.cert.clone().into()), Some(cfg.key.clone().into()))
                .unwrap()
                .expect("both set gives Some");
        assert_eq!(got, cfg);
    }

    #[test]
    fn from_env_errors_when_only_cert_set() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_fixture(dir.path());
        let err = PublicTlsConfig::from_vars(Some(cfg.cert.into()), None).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains(CERT_VAR), "{text}");
        assert!(text.contains(KEY_VAR), "{text}");
    }

    #[test]
    fn from_env_errors_when_only_key_set() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_fixture(dir.path());
        let err = PublicTlsConfig::from_vars(None, Some(cfg.key.into())).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains(CERT_VAR), "{text}");
        assert!(text.contains(KEY_VAR), "{text}");
    }

    #[test]
    fn from_env_errors_on_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_fixture(dir.path());
        let missing = dir.path().join("no-such.crt");
        let err = PublicTlsConfig::from_vars(Some(missing.clone().into()), Some(cfg.key.into()))
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains(&missing.display().to_string()), "{text}");
    }

    #[test]
    fn server_config_offers_h2_and_http11() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_fixture(dir.path());
        let server = cfg.rustls_server_config().unwrap();
        assert_eq!(
            server.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }
}
// HANDWRITE-END
