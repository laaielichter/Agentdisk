//! Core of agentdisk: scanning, accounting and safety rules.
//! Step 0.0.1 only exposes the version.

/// Version of agentdisk, taken from Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!VERSION.is_empty());
    }
}
