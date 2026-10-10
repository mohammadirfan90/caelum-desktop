//! Mutual TLS configuration (CR-017 §2, §4): TLS 1.3 only, the `ring` provider, the crew-LAN
//! certificate authority as the only trust anchor, and a client certificate required on every
//! connection.

use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use super::{fingerprint, Fingerprint};

/// A TLS configuration failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tls: {0}")]
pub struct TlsError(pub String);

fn err(e: impl std::fmt::Display) -> TlsError {
    TlsError(e.to_string())
}

/// This station's certificate and key, and the crew-LAN certificate authority.
#[derive(Debug)]
pub struct TlsMaterial {
    ca: CertificateDer<'static>,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl TlsMaterial {
    /// From DER: the CA certificate, this station's chain (leaf first) and private key.
    pub fn from_der(
        ca: CertificateDer<'static>,
        chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> Result<Self, TlsError> {
        if chain.is_empty() {
            return Err(TlsError("empty certificate chain".into()));
        }
        Ok(Self { ca, chain, key })
    }

    /// From PEM text.
    pub fn from_pem(ca_pem: &str, chain_pem: &str, key_pem: &str) -> Result<Self, TlsError> {
        let ca = CertificateDer::from_pem_slice(ca_pem.as_bytes()).map_err(err)?;
        let chain = CertificateDer::pem_slice_iter(chain_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(err)?;
        Self::from_der(ca, chain, key)
    }

    /// Fingerprint of this station's own certificate (for the peer's registry).
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.chain.first().map(|c| fingerprint(c))
    }

    fn roots(&self) -> Result<Arc<RootCertStore>, TlsError> {
        let mut roots = RootCertStore::empty();
        roots.add(self.ca.clone()).map_err(err)?;
        Ok(Arc::new(roots))
    }

    /// Server side: requires a client certificate issued by the crew-LAN CA.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, TlsError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = WebPkiClientVerifier::builder_with_provider(self.roots()?, provider.clone())
            .build()
            .map_err(err)?;
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(err)?
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.chain.clone(), self.key.clone_key())
            .map_err(err)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }

    /// Client side: trusts only the crew-LAN CA and presents this station's certificate.
    pub fn client_config(&self) -> Result<Arc<ClientConfig>, TlsError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(err)?
            .with_root_certificates(self.roots()?)
            .with_client_auth_cert(self.chain.clone(), self.key.clone_key())
            .map_err(err)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}
