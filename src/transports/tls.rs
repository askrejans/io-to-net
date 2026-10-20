//! rustls server configuration from PEM files, with optional client
//! certificate verification (mutual TLS).

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;

use crate::config::TlsTransportConfig;

fn certs(path: &str) -> Result<Vec<CertificateDer<'static>>> {
    let certs = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("cannot read {path}"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("invalid PEM in {path}"))?;
    if certs.is_empty() {
        bail!("no certificates in {path}");
    }
    Ok(certs)
}

pub fn server_config(cfg: &TlsTransportConfig) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let chain = certs(&cfg.cert)?;
    // PKCS#8, PKCS#1 (RSA) and SEC1 (EC) keys are all accepted.
    let key = PrivateKeyDer::from_pem_file(&cfg.key)
        .with_context(|| format!("no private key in {}", cfg.key))?;
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let builder = match &cfg.client_ca {
        None => builder.with_no_client_auth(),
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for c in certs(ca)? {
                roots
                    .add(c)
                    .with_context(|| format!("bad CA certificate in {ca}"))?;
            }
            builder.with_client_cert_verifier(
                WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                    .build()
                    .context("client CA")?,
            )
        }
    };
    let config = builder
        .with_single_cert(chain, key)
        .context("certificate and key do not match")?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(dir: &std::path::Path) -> TlsTransportConfig {
        crate::certgen::generate_cert(dir, "localhost", 1).unwrap();
        TlsTransportConfig {
            cert: dir.join("cert.pem").display().to_string(),
            key: dir.join("key.pem").display().to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn loads_generated_certificate() {
        let dir = tempfile::tempdir().unwrap();
        assert!(server_config(&cfg(dir.path())).is_ok());
    }

    #[test]
    fn mutual_tls_with_ca_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = cfg(dir.path());
        c.client_ca = Some(c.cert.clone());
        assert!(server_config(&c).is_ok());
    }

    #[test]
    fn missing_or_empty_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = cfg(dir.path());
        std::fs::write(dir.path().join("empty.pem"), "").unwrap();
        c.key = dir.path().join("empty.pem").display().to_string();
        assert!(server_config(&c).is_err());
        c.cert = "/nonexistent/cert.pem".into();
        assert!(server_config(&c).is_err());
    }
}
