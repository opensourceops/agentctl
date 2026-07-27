use std::net::SocketAddr;
use std::time::Duration;

use rustls_pki_types::pem::{PemObject, SectionKind};

use crate::secret::SecretValue;

pub const DEFAULT_NETWORK_CONNECT_TIMEOUT_SECONDS: u64 = 10;
pub const DEFAULT_NETWORK_RESPONSE_LIMIT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_NETWORK_CONNECT_TIMEOUT_SECONDS: u64 = 120;
pub const MAX_NETWORK_RESPONSE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct HttpTransportSecurity {
    pub resolved_host: Option<String>,
    pub resolved_addresses: Vec<SocketAddr>,
    pub allow_proxy: bool,
    pub connect_timeout: Duration,
    pub max_response_bytes: usize,
    pub custom_ca_pem: Option<SecretValue>,
}

impl Default for HttpTransportSecurity {
    fn default() -> Self {
        Self {
            resolved_host: None,
            resolved_addresses: Vec::new(),
            allow_proxy: false,
            connect_timeout: Duration::from_secs(DEFAULT_NETWORK_CONNECT_TIMEOUT_SECONDS),
            max_response_bytes: usize::try_from(DEFAULT_NETWORK_RESPONSE_LIMIT_BYTES)
                .unwrap_or(usize::MAX),
            custom_ca_pem: None,
        }
    }
}

#[must_use]
pub fn custom_ca_pem_is_valid(pem: &str) -> bool {
    let mut found_certificate = false;
    for item in <(SectionKind, Vec<u8>)>::pem_slice_iter(pem.as_bytes()) {
        match item {
            Ok((SectionKind::Certificate, _)) => {
                found_certificate = true;
            }
            Ok(_) | Err(_) => return false,
        }
    }
    found_certificate
}
