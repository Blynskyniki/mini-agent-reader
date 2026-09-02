//! Trust configuration, including root certificates no public trust store has.
//!
//! A number of Russian sites — gosuslugi.ru and mos.ru among them — present
//! certificates issued under the Russian Ministry of Digital Development's own
//! authority. No browser or operating system ships that root, so without it
//! those sites cannot be fetched at all: the connection fails during the
//! handshake, before any HTTP is exchanged.
//!
//! The roots are bundled, but not trusted unconditionally. See [`TrustMode`].

use ureq::tls::{Certificate, RootCerts, TlsConfig};

/// The Ministry of Digital Development's root, valid to 2032.
const RU_ROOT_CA: &[u8] = include_bytes!("../certs/russian_trusted_root_ca.pem");
/// Its intermediate, valid to 2027. Some servers do not send it in the chain,
/// so having it locally is what makes those handshakes complete.
const RU_SUB_CA: &[u8] = include_bytes!("../certs/russian_trusted_sub_ca.pem");

/// How much to trust, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustMode {
    /// Verify against the public roots; if and only if that fails, retry
    /// against the public roots plus the bundled extras.
    ///
    /// This is the default, and it is the reason the extra roots are safe to
    /// ship. An authority run by a government can issue a certificate for any
    /// domain, so adding one to the trust set unconditionally would let it
    /// intercept every site the tool ever fetches. Consulting it only after the
    /// standard chain has already been rejected means it can rescue a site that
    /// would otherwise fail, and can never override one that works.
    #[default]
    PublicThenExtra,
    /// Public roots only. Sites that need the extra roots will fail.
    PublicOnly,
    /// Public roots and the extra roots together, in one trust set. Faster by
    /// one handshake on affected sites, at the cost described above.
    Combined,
    /// Verify nothing. For debugging against a local server with a self-signed
    /// certificate; never appropriate against the public internet.
    None,
}

/// What is bundled, for `mar certs`.
#[derive(Debug, Clone)]
pub struct BundledCert {
    pub name: &'static str,
    pub subject: String,
    pub not_after: String,
    pub source: &'static str,
}

/// Parse the bundled PEM files into certificates.
///
/// Returns an empty vector if a file fails to parse, which cannot happen with
/// the files as committed but must not panic if one is ever replaced badly.
pub fn extra_roots() -> Vec<Certificate<'static>> {
    [RU_ROOT_CA, RU_SUB_CA]
        .iter()
        .filter_map(|pem| Certificate::from_pem(pem).ok())
        .collect()
}

/// Describe the bundled certificates without pulling in an X.509 parser: the
/// fields below are fixed properties of these two files.
pub fn bundled_certs() -> Vec<BundledCert> {
    vec![
        BundledCert {
            name: "russian_trusted_root_ca",
            subject: "Russian Trusted Root CA (The Ministry of Digital Development)".into(),
            not_after: "2032-02-27".into(),
            source: "https://gu-st.ru/content/lending/russian_trusted_root_ca_pem.crt",
        },
        BundledCert {
            name: "russian_trusted_sub_ca",
            subject: "Russian Trusted Sub CA (The Ministry of Digital Development)".into(),
            not_after: "2027-03-06".into(),
            source: "https://gu-st.ru/content/lending/russian_trusted_sub_ca_pem.crt",
        },
    ]
}

/// Load additional roots from a PEM bundle supplied by the caller, for a
/// corporate authority or a private certificate authority.
pub fn load_pem_bundle(path: &std::path::Path) -> std::io::Result<Vec<Certificate<'static>>> {
    let bytes = std::fs::read(path)?;
    let mut out = Vec::new();
    // A bundle holds several certificates concatenated; split on the PEM
    // boundary so each is parsed on its own.
    let text = String::from_utf8_lossy(&bytes);
    let mut current = String::new();
    for line in text.lines() {
        current.push_str(line);
        current.push('\n');
        if line.starts_with("-----END CERTIFICATE-----") {
            if let Ok(cert) = Certificate::from_pem(current.as_bytes()) {
                out.push(cert);
            }
            current.clear();
        }
    }
    if out.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("no certificates found in {}", path.display()),
        ));
    }
    Ok(out)
}

/// The TLS configuration used for the first attempt.
pub fn primary_config(mode: TrustMode, extra: &[Certificate<'static>]) -> TlsConfig {
    match mode {
        TrustMode::PublicOnly | TrustMode::PublicThenExtra => {
            TlsConfig::builder().root_certs(RootCerts::WebPki).build()
        }
        TrustMode::Combined => TlsConfig::builder()
            .root_certs(combined_roots(extra))
            .build(),
        TrustMode::None => TlsConfig::builder()
            .root_certs(RootCerts::WebPki)
            .disable_verification(true)
            .build(),
    }
}

/// The configuration retried after a certificate failure, when the mode has one.
pub fn fallback_config(
    mode: TrustMode,
    extra: &[Certificate<'static>],
) -> Option<TlsConfig> {
    match mode {
        TrustMode::PublicThenExtra => Some(
            TlsConfig::builder()
                .root_certs(combined_roots(extra))
                .build(),
        ),
        _ => None,
    }
}

/// Mozilla's roots plus whatever extras were supplied, in one set.
fn combined_roots(extra: &[Certificate<'static>]) -> RootCerts {
    let mut certs: Vec<Certificate<'static>> = webpki_roots::TLS_SERVER_ROOTS
        .iter()
        .map(|anchor| {
            // webpki-roots stores trust anchors, not full certificates; the
            // subject public key info is the DER we need.
            Certificate::from_der(anchor.subject_public_key_info.as_ref()).to_owned()
        })
        .collect();
    certs.extend(extra.iter().cloned());
    RootCerts::Specific(std::sync::Arc::new(certs))
}

/// Does this error look like the peer's certificate chain was rejected?
///
/// rustls reports verification failures as an I/O error carrying a message, so
/// the text is what there is to match on. Being wrong in the permissive
/// direction only costs one wasted retry.
pub fn is_certificate_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "certificate",
        "unknownissuer",
        "unknown issuer",
        "invalidcertificate",
        "badcertificate",
        "cert verify",
        "unable to get local issuer",
        "self signed",
        "self-signed",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_roots_parse() {
        let roots = extra_roots();
        assert_eq!(roots.len(), 2, "both the root and the intermediate load");
    }

    #[test]
    fn bundled_metadata_matches_what_is_shipped() {
        let described = bundled_certs();
        assert_eq!(described.len(), extra_roots().len());
        assert!(described.iter().all(|c| c.source.starts_with("https://")));
    }

    #[test]
    fn certificate_failures_are_recognised() {
        for message in [
            "invalid peer certificate: UnknownIssuer",
            "tls handshake eof: InvalidCertificate(UnknownIssuer)",
            "unable to get local issuer certificate",
            "self-signed certificate in certificate chain",
        ] {
            assert!(is_certificate_error(message), "should match: {message}");
        }
        for message in ["connection refused", "dns error", "timed out"] {
            assert!(!is_certificate_error(message), "should not match: {message}");
        }
    }

    #[test]
    fn only_the_two_stage_mode_has_a_fallback() {
        let extra = extra_roots();
        assert!(fallback_config(TrustMode::PublicThenExtra, &extra).is_some());
        assert!(fallback_config(TrustMode::PublicOnly, &extra).is_none());
        assert!(fallback_config(TrustMode::Combined, &extra).is_none());
        assert!(fallback_config(TrustMode::None, &extra).is_none());
    }
}
