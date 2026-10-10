//! The FHIR REST operations CAELUM uses, as a trait (ICD-MDS §3–4, §9; R-INT-05). The connector is
//! sans-I/O: the HTTPS (TLS 1.3, mutual certificates) implementation is supplied by the core
//! (CR-016); for tests, `FakeFhirServer` in `crate::fake` (test equipment only).
//!
//! The interface is read/create only: there is no update, delete or operation call, so no code
//! path can command the medical data system (ICD-MDS §7: no commands in either direction).

use serde_json::Value;

use crate::ids::ClientId;

/// A transport failure, classified for the outbox retry policy (ICD-MDS §9).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Could not connect (link down, refused, TLS failure).
    #[error("medical data system unreachable: {0}")]
    Unreachable(String),
    /// No complete response in time; the request may or may not have been applied.
    #[error("request timed out")]
    Timeout,
    /// The server rejected the request (4xx), with the `OperationOutcome` text if any.
    #[error("HTTP {status}: {outcome}")]
    Http4xx {
        /// HTTP status.
        status: u16,
        /// `OperationOutcome` diagnostics, or the reason phrase.
        outcome: String,
    },
    /// The server failed (5xx).
    #[error("HTTP {0}")]
    Http5xx(u16),
    /// The response was not valid HTTP / FHIR JSON.
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl TransportError {
    /// Whether the request should be retried with back-off. 4xx is held for review except 408
    /// (request timeout) and 429 (too many requests), which are transient.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http4xx { status, .. } => matches!(status, 408 | 429),
            _ => true,
        }
    }

    /// Whether the exchange still proves a working connection (for health): the server processed
    /// the request and answered 4xx. A 5xx, timeout or link failure does not.
    pub fn proves_contact(&self) -> bool {
        matches!(self, Self::Http4xx { .. })
    }
}

/// Result of a conditional create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOutcome {
    /// The server's logical id of the resource.
    pub server_id: String,
    /// True when created now (201); false when a resource with this client id already existed
    /// (200), e.g. a retry after a lost response.
    pub created: bool,
}

/// A search on `_lastUpdated` for read-back polling (ICD-MDS §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Resource type, e.g. `Observation`.
    pub resource_type: String,
    /// `_lastUpdated=ge<instant>`; `None` reads from the beginning.
    pub last_updated_ge: Option<String>,
    /// `_count` page size; results are sorted by `_lastUpdated`.
    pub count: u32,
}

/// FHIR R4 REST operations used by the connector.
pub trait FhirTransport {
    /// `POST [base]/<resource_type>` with `If-None-Exist: identifier=urn:caelum:client-id|<id>`.
    fn create_conditional(
        &mut self,
        resource_type: &str,
        client_id: ClientId,
        resource: &Value,
    ) -> Result<CreateOutcome, TransportError>;

    /// `GET [base]/<type>?_lastUpdated=ge..&_sort=_lastUpdated&_count=..`; returns the matching
    /// resources (bundle entries) in `_lastUpdated` order.
    fn search(&mut self, query: &SearchQuery) -> Result<Vec<Value>, TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ICD-MDS §9: 4xx is held for review (except 408/429); everything else is retried.
    #[test]
    fn icd_mds_9_retry_classification() {
        let c4 = |status| TransportError::Http4xx {
            status,
            outcome: String::new(),
        };
        assert!(!c4(400).is_retryable());
        assert!(!c4(422).is_retryable());
        assert!(c4(408).is_retryable());
        assert!(c4(429).is_retryable());
        assert!(TransportError::Http5xx(503).is_retryable());
        assert!(TransportError::Timeout.is_retryable());
        assert!(TransportError::Unreachable("x".into()).is_retryable());
        assert!(TransportError::Protocol("x".into()).is_retryable());
        assert!(c4(400).proves_contact());
        assert!(!TransportError::Http5xx(500).proves_contact());
        assert!(!TransportError::Timeout.proves_contact());
    }
}
