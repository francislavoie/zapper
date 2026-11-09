//! HTTPS for other devices: the service's own self-signed certificate.
//!
//! Made the first time network access with HTTPS is turned on and kept in
//! the config directory (`server-cert.pem`, and `server-key.pem` readable by
//! this user only), so it stays the same across restarts and a browser that
//! was told to trust it once keeps trusting it. No authority vouches for it,
//! so browsers warn the first time; Settings shows its SHA-256 fingerprint,
//! which is what the browser's warning shows too, to check that the
//! certificate is the service's and not someone else's in between.
//!
//! Deleting the two files makes a new one on the next start (with a new
//! fingerprint).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};

use zapper_core::recording::config::write_private;

pub fn cert_path(dir: &Path) -> PathBuf {
    dir.join("server-cert.pem")
}

pub fn key_path(dir: &Path) -> PathBuf {
    dir.join("server-key.pem")
}

/// A certificate and its private key, as DER.
pub struct Identity {
    pub cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.cert)
    }
}

/// SHA-256 of the certificate as browsers show it: upper-case hex pairs
/// joined by colons.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The certificate on file, if there is a readable one.
pub fn load(dir: &Path) -> Option<Identity> {
    let cert = std::fs::read(cert_path(dir)).ok()?;
    let key = std::fs::read(key_path(dir)).ok()?;
    let cert = CertificateDer::from_pem_slice(&cert).ok()?;
    let key = PrivateKeyDer::from_pem_slice(&key).ok()?;
    Some(Identity { cert, key })
}

/// The certificate on file, or a new one made and kept there. `names` are
/// the host names and addresses it is for (any parseable as an address
/// becomes an IP entry).
pub fn load_or_create(dir: &Path, names: &[String]) -> Result<Identity> {
    if let Some(id) = load(dir) {
        return Ok(id);
    }
    if cert_path(dir).exists() || key_path(dir).exists() {
        tracing::warn!(
            "the HTTPS certificate in {} could not be read; making a new one",
            dir.display()
        );
    }
    let (cert_pem, key_pem) = create(names)?;
    // The key first: a certificate on file without its key is no use, and
    // is replaced next time like a missing one.
    write_private(&key_path(dir), key_pem.as_bytes()).context("saving the HTTPS key")?;
    write_private(&cert_path(dir), cert_pem.as_bytes()).context("saving the HTTPS certificate")?;
    load(dir).ok_or_else(|| anyhow!("the new HTTPS certificate could not be read back"))
}

/// A new self-signed certificate (ECDSA P-256) for `names`, as PEM: the
/// certificate, then the key. Valid from yesterday, so a clock a little
/// behind accepts it, for ten years: browsers do not limit the lifetime of
/// a certificate that a person chose to trust.
pub fn create(names: &[String]) -> Result<(String, String)> {
    let mut names: Vec<String> = names.to_vec();
    if names.is_empty() {
        names.push("localhost".into());
    }
    let mut params =
        rcgen::CertificateParams::new(names).map_err(|e| anyhow!("certificate names: {e}"))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "zapper recording service");
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "zapper");
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
    let key = rcgen::KeyPair::generate().map_err(|e| anyhow!("making the HTTPS key: {e}"))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| anyhow!("making the HTTPS certificate: {e}"))?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// What to serve TLS with. HTTP/1.1 only, as the rest of the service.
pub fn acceptor(id: &Identity) -> Result<tokio_rustls::TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| anyhow!("TLS: {e}"))?
        .with_no_client_auth()
        .with_single_cert(vec![id.cert.clone()], id.key.clone_key())
        .map_err(|e| anyhow!("TLS: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// The names a new certificate is made for: `localhost`, this computer's
/// host name, and the addresses it has now. An address that changes later
/// only adds a name mismatch to the warning a self-signed certificate gets
/// anyway.
pub fn names_for(addresses: &[std::net::IpAddr]) -> Vec<String> {
    let mut names = vec!["localhost".to_string()];
    if let Some(h) = host_name() {
        if !names.contains(&h) {
            names.push(h);
        }
    }
    names.push("127.0.0.1".into());
    names.push("::1".into());
    for a in addresses {
        let a = a.to_string();
        if !names.contains(&a) {
            names.push(a);
        }
    }
    names
}

/// This computer's name, when it is one a certificate can carry.
pub fn host_name() -> Option<String> {
    #[cfg(unix)]
    let name = {
        let mut buf = [0u8; 256];
        // SAFETY: a buffer and its length for gethostname to fill.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    };
    #[cfg(not(unix))]
    let name = std::env::var("COMPUTERNAME").ok()?;
    let name = name.trim().to_ascii_lowercase();
    let ok = !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zapper-tls-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_fingerprint_reads_like_a_browsers() {
        // SHA-256 of nothing.
        assert_eq!(
            fingerprint(b""),
            "E3:B0:C4:42:98:FC:1C:14:9A:FB:F4:C8:99:6F:B9:24:27:AE:41:E4:64:9B:93:4C:A4:95:99:1B:78:52:B8:55"
        );
    }

    #[test]
    fn the_certificate_is_made_once_and_kept() {
        let d = scratch("keep");
        assert!(load(&d).is_none());
        let names = names_for(&["192.168.1.20".parse().unwrap()]);
        let first = load_or_create(&d, &names).unwrap();
        let fp = first.fingerprint();
        assert_eq!(fp.len(), 32 * 3 - 1);
        assert!(cert_path(&d).is_file() && key_path(&d).is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(key_path(&d))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the key is private");
        }
        let pem = std::fs::read_to_string(cert_path(&d)).unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
        // The same one next time.
        assert_eq!(load_or_create(&d, &names).unwrap().fingerprint(), fp);
        assert_eq!(load(&d).unwrap().fingerprint(), fp);
        // It serves.
        acceptor(&first).unwrap();
        // A broken pair is replaced.
        std::fs::write(key_path(&d), "garbage").unwrap();
        let new = load_or_create(&d, &names).unwrap();
        assert_ne!(new.fingerprint(), fp);
        // Gone: a new one too.
        std::fs::remove_file(cert_path(&d)).unwrap();
        std::fs::remove_file(key_path(&d)).unwrap();
        assert_ne!(
            load_or_create(&d, &names).unwrap().fingerprint(),
            new.fingerprint()
        );
    }

    #[test]
    fn it_names_this_computer_and_its_addresses() {
        let names = names_for(&["192.168.1.20".parse().unwrap(), "fd00::5".parse().unwrap()]);
        assert_eq!(names[0], "localhost");
        for n in ["127.0.0.1", "::1", "192.168.1.20", "fd00::5"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        // Every one is a name rcgen takes.
        create(&names).unwrap();
    }
}
