use std::path::Path;

use anyhow::{Context, Result};
use chrono::Datelike;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use tracing::info;

/// Generate a self-signed TLS certificate + private key and write them to `out_dir/cert.pem`
/// and `out_dir/key.pem`.
pub fn generate_cert(out_dir: &Path, cn: &str, days: u32) -> Result<()> {
    // Create output directory if it doesn't exist
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("Cannot create output directory: {}", out_dir.display()))?;

    let mut params = CertificateParams::default();

    // Distinguished name
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    dn.push(DnType::OrganizationName, "io-to-net");
    params.distinguished_name = dn;

    // Subject alternative names
    params.subject_alt_names = vec![
        SanType::DnsName(
            cn.try_into()
                .unwrap_or_else(|_| "io-to-net".try_into().unwrap()),
        ),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    ];

    // Validity period
    let now = chrono::Utc::now();
    let future = now + chrono::Duration::days(days as i64);

    params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    params.not_after =
        rcgen::date_time_ymd(future.year(), future.month() as u8, future.day() as u8);

    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    let cert_path = out_dir.join("cert.pem");
    let key_path = out_dir.join("key.pem");

    std::fs::write(&cert_path, &cert_pem)
        .with_context(|| format!("Cannot write cert to {}", cert_path.display()))?;
    std::fs::write(&key_path, key_pem.as_bytes())
        .with_context(|| format!("Cannot write key to {}", key_path.display()))?;

    // Restrict key file permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| "Cannot set key file permissions")?;
    }

    info!(cert = %cert_path.display(), key = %key_path.display(), cn = cn, days = days, "Certificate generated");
    println!("Certificate written to: {}", cert_path.display());
    println!("Private key written to: {}", key_path.display());
    println!("CN: {}  |  Validity: {} days", cn, days);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn run(cn: &str, days: u32) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        generate_cert(dir.path(), cn, days).unwrap();
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        (dir, cert, key)
    }

    #[test]
    fn files_created() {
        let (_dir, cert, key) = run("test.local", 90);
        assert!(cert.exists(), "cert.pem missing");
        assert!(key.exists(), "key.pem missing");
    }

    #[test]
    fn cert_pem_starts_with_header() {
        let (_dir, cert, _key) = run("io-to-net", 30);
        let content = std::fs::read_to_string(cert).unwrap();
        assert!(content.contains("-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn key_pem_starts_with_header() {
        let (_dir, _cert, key) = run("io-to-net", 30);
        let content = std::fs::read_to_string(key).unwrap();
        assert!(
            content.contains("-----BEGIN PRIVATE KEY-----")
                || content.contains("-----BEGIN EC PRIVATE KEY-----")
        );
    }

    #[test]
    fn idempotent_on_existing_dir() {
        let dir = TempDir::new().unwrap();
        generate_cert(dir.path(), "a.local", 10).unwrap();
        // Second write should overwrite without error
        generate_cert(dir.path(), "b.local", 10).unwrap();
        let content = std::fs::read_to_string(dir.path().join("cert.pem")).unwrap();
        assert!(content.contains("-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn creates_nested_output_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        generate_cert(&nested, "deep.local", 7).unwrap();
        assert!(nested.join("cert.pem").exists());
    }

    #[cfg(unix)]
    #[test]
    fn key_file_has_restricted_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, _cert, key) = run("perm.test", 1);
        let meta = std::fs::metadata(&key).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key should be 0600, got {:o}", mode);
    }
}
