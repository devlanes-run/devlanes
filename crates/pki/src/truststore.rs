//! PKCS#12 trust stores for JVMs, which cannot read PEM bundles.

use std::collections::BTreeSet;

use p12_keystore::{Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, MacAlgorithm};

use crate::Error;

/// Java's default PBE iteration count for PKCS#12 stores.
const ITERATIONS: u32 = 10_000;

/// A PKCS#12 trust store with the certificates it had to leave out.
#[derive(Debug, Clone)]
pub struct TrustStore {
    /// The encoded store, ready to write to disk.
    pub pkcs12: Vec<u8>,
    /// One line per PEM block or certificate that could not be read and was skipped.
    pub problems: Vec<String>,
}

/// Builds a PKCS#12 trust store from PEM roots, such as the input of
/// [`crate::CertificateAuthority::combined_bundle`], protected by `password`.
///
/// Every certificate becomes a trusted-certificate entry (the Oracle
/// trusted-key-usage attribute Java requires), identical certificates are stored
/// once, and certificates that fail to parse are listed in
/// [`TrustStore::problems`] instead of failing the store. The legacy 3DES and
/// HMAC-SHA1 algorithms keep it readable by JDK 8 before 8u301 and JDK 11 before
/// 11.0.12, which reject the AES and SHA-256 defaults.
///
/// # Errors
///
/// [`Error::EmptyTrustStore`] when no certificate could be read, and
/// [`Error::TrustStore`] when encoding fails.
pub fn pkcs12_trust_store(pem_roots: &str, password: &str) -> Result<TrustStore, Error> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    let mut store = KeyStore::new();
    let mut reader = pem_roots.as_bytes();
    for (index, block) in rustls_pemfile::certs(&mut reader).enumerate() {
        let number = index + 1;
        let der = match block {
            Ok(der) => der,
            Err(error) => {
                problems.push(format!("certificate {number}: unreadable PEM: {error}"));
                continue;
            }
        };
        if !seen.insert(der.as_ref().to_vec()) {
            continue;
        }
        let certificate = match Certificate::from_der(der.as_ref()) {
            Ok(certificate) => certificate,
            Err(error) => {
                problems.push(format!("certificate {number}: {error}"));
                continue;
            }
        };
        let alias = unique_alias(&mut aliases, &base_alias(der.as_ref(), &certificate));
        store.add_entry(&alias, KeyStoreEntry::Certificate(certificate));
    }
    if store.entries_len() == 0 {
        return Err(Error::EmptyTrustStore);
    }
    let pkcs12 = store
        .writer(password)
        .encryption_algorithm(EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc)
        .encryption_iterations(ITERATIONS)
        .mac_algorithm(MacAlgorithm::HmacSha1)
        .mac_iterations(ITERATIONS)
        .write()?;
    Ok(TrustStore { pkcs12, problems })
}

/// The certificate's common name, else its whole subject, as Java would list it.
///
/// Java lowercases PKCS#12 aliases and stores them as BMPString, so the alias is
/// lowercased here and characters outside the Basic Multilingual Plane are dropped.
fn base_alias(der: &[u8], certificate: &Certificate) -> String {
    let common_name = x509_parser::parse_x509_certificate(der)
        .ok()
        .and_then(|(_, parsed)| {
            parsed
                .subject()
                .iter_common_name()
                .next()
                .and_then(|cn| cn.as_str().ok())
                .map(str::to_owned)
        });
    let name = common_name.unwrap_or_else(|| certificate.subject().to_owned());
    let alias: String = name
        .chars()
        .filter(|c| u32::from(*c) <= 0xffff)
        .collect::<String>()
        .trim()
        .to_lowercase();
    if alias.is_empty() {
        "root".to_owned()
    } else {
        alias
    }
}

/// `base`, or `base N` with the smallest N from 2 that is not taken yet.
fn unique_alias(taken: &mut BTreeSet<String>, base: &str) -> String {
    let mut alias = base.to_owned();
    let mut suffix = 2;
    while taken.contains(&alias) {
        alias = format!("{base} {suffix}");
        suffix += 1;
    }
    taken.insert(alias.clone());
    alias
}
