use std::fmt;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::dsl::SecretReference;

/// A resolved secret held only in memory and zeroized when dropped.
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> Self {
        Self::new(self.expose().to_owned())
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl From<String> for SecretValue {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretValue {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

/// Runtime-supplied secret resolution used by provider adapters at dispatch time.
#[async_trait]
pub trait SecretSourceResolver: fmt::Debug + Send + Sync {
    /// Resolve a reference without persisting or logging the returned value.
    ///
    /// Error messages must describe only the source and failure, never the value.
    async fn resolve_secret(
        &self,
        reference: &SecretReference,
        cancellation: &CancellationToken,
    ) -> Result<SecretValue, String>;
}

#[cfg(test)]
mod tests {
    use super::SecretValue;

    #[test]
    fn debug_never_exposes_the_value() {
        let value = SecretValue::from("fixture-secret");
        assert_eq!(format!("{value:?}"), "SecretValue([REDACTED])");
        assert_eq!(value.expose(), "fixture-secret");
    }
}
