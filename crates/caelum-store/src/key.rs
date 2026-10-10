//! The database key: 256 bits, wiped from memory when dropped, never printed (R-SEC-01).

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// A 256-bit SQLCipher raw key.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct DbKey([u8; 32]);

impl DbKey {
    /// Wraps 32 key bytes (from the keystore or a hardware token).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The SQLCipher raw-key pragma value `x'…'`, wiped when dropped.
    pub(crate) fn pragma_value(&self) -> Zeroizing<String> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = Zeroizing::new(String::with_capacity(67));
        s.push_str("x'");
        for byte in &self.0 {
            s.push(char::from(HEX[usize::from(byte >> 4)]));
            s.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        s.push('\'');
        s
    }
}

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R-SEC-01: the key never appears in debug output.
    #[test]
    fn key_is_redacted_in_debug_output() {
        let key = DbKey::from_bytes([0xab; 32]);
        assert_eq!(format!("{key:?}"), "DbKey(<redacted>)");
    }

    #[test]
    fn pragma_value_is_raw_hex_key() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0x01;
        bytes[31] = 0xff;
        let v = DbKey::from_bytes(bytes).pragma_value();
        assert_eq!(v.len(), 67);
        assert!(v.starts_with("x'01") && v.ends_with("ff'"));
    }
}
