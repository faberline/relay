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
//!
pub mod public;

/// Env prefix of the peer-mTLS material: `peer_tls::PeerTlsConfig::from_env`
/// reads `<prefix>_TLS_CERT`, `<prefix>_TLS_KEY`, `<prefix>_TLS_CA` and `<prefix>_MTLS`.
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
    use super::PEER_ENV_PREFIX;

    #[test]
    fn peer_env_prefix_keeps_the_relay_contract() {
        assert_eq!(PEER_ENV_PREFIX, "RELAY_PEER");
        assert_eq!(format!("{PEER_ENV_PREFIX}_TLS_CERT"), "RELAY_PEER_TLS_CERT");
    }
}
// HANDWRITE-END
