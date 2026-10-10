//! Pairing and bonding (F-ACQ-01): platform modules, with guided OS pairing as the fallback.
//!
//! - Windows: WinRT `DeviceInformationPairing` with at least encryption; the system shows its own
//!   prompt when a device needs a passkey.
//! - Linux: BlueZ through `bluer` (`Device::pair`).
//! - macOS: CoreBluetooth pairs by itself the first time an encrypted characteristic is read; there
//!   is no separate step, so CAELUM reports that and offers the guide.
//!
//! When pairing cannot complete here (passkey devices the platform will not prompt for, timeouts,
//! refused), the outcome is [`PairingOutcome::NeedsOsPairing`] and the UI shows [`os_pairing_guide`].
//! Pairing is link-layer security handled by the OS; it writes no GATT value.

/// A Bluetooth device address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BtAddress(pub [u8; 6]);

impl BtAddress {
    /// Parses `AA:BB:CC:DD:EE:FF` (case-insensitive).
    pub fn parse(text: &str) -> Option<Self> {
        let mut out = [0u8; 6];
        let mut parts = text.split(':');
        for byte in &mut out {
            let part = parts.next()?;
            if part.len() != 2 {
                return None;
            }
            *byte = u8::from_str_radix(part, 16).ok()?;
        }
        parts.next().is_none().then_some(Self(out))
    }

    /// The address as the 48-bit integer WinRT uses (most significant byte first).
    pub fn as_u64(self) -> u64 {
        self.0
            .iter()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
    }
}

impl std::fmt::Display for BtAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

/// Result of a pairing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingOutcome {
    /// Paired and bonded now.
    Paired,
    /// Was already paired.
    AlreadyPaired,
    /// The platform pairs on first encrypted access (macOS); nothing to do now.
    OnFirstAccess,
    /// Pairing has to be done in the OS settings (guided fallback); the reason is shown.
    NeedsOsPairing(String),
}

/// Guided OS pairing, per platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsPairingGuide {
    /// Steps shown to the crew member.
    pub steps: &'static [&'static str],
    /// Settings page the app can open, if the platform has one.
    pub settings_uri: Option<&'static str>,
}

/// The guide for this platform.
pub fn os_pairing_guide() -> OsPairingGuide {
    if cfg!(windows) {
        OsPairingGuide {
            steps: &[
                "Put the device in pairing mode (see its instructions).",
                "In Windows Settings, open Bluetooth & devices and choose Add device, then Bluetooth.",
                "Select the device and enter or confirm the code it shows.",
                "Return to CAELUM and select Retry.",
            ],
            settings_uri: Some("ms-settings:bluetooth"),
        }
    } else if cfg!(target_os = "macos") {
        OsPairingGuide {
            steps: &[
                "Put the device in pairing mode (see its instructions).",
                "Open System Settings, then Bluetooth, and select Connect next to the device.",
                "Enter or confirm the code it shows, then return to CAELUM and select Retry.",
            ],
            settings_uri: Some("x-apple.systempreferences:com.apple.BluetoothSettings"),
        }
    } else {
        OsPairingGuide {
            steps: &[
                "Put the device in pairing mode (see its instructions).",
                "Open the Bluetooth settings of the desktop, or run `bluetoothctl`, then `pair <address>`.",
                "Enter or confirm the code it shows, then return to CAELUM and select Retry.",
            ],
            settings_uri: None,
        }
    }
}

/// Pairing errors (the platform could not even try).
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// Platform failure.
    #[error("pairing: {0}")]
    Platform(String),
}

/// Pairs with the device at `address` (encryption required).
pub async fn pair(address: BtAddress) -> Result<PairingOutcome, PairingError> {
    platform::pair(address).await
}

#[cfg(windows)]
mod platform {
    use windows::Devices::Bluetooth::BluetoothLEDevice;
    use windows::Devices::Enumeration::{
        DevicePairingProtectionLevel, DevicePairingResultStatus as S,
    };

    use super::{BtAddress, PairingError, PairingOutcome};

    fn platform(e: windows::core::Error) -> PairingError {
        PairingError::Platform(e.message())
    }

    pub(super) async fn pair(address: BtAddress) -> Result<PairingOutcome, PairingError> {
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(address.as_u64())
            .map_err(platform)?
            .await
            .map_err(platform)?;
        let pairing = device
            .DeviceInformation()
            .map_err(platform)?
            .Pairing()
            .map_err(platform)?;
        if pairing.IsPaired().map_err(platform)? {
            return Ok(PairingOutcome::AlreadyPaired);
        }
        if !pairing.CanPair().map_err(platform)? {
            return Ok(PairingOutcome::NeedsOsPairing(
                "the device is not in pairing mode or does not accept pairing from apps".into(),
            ));
        }
        let result = pairing
            .PairWithProtectionLevelAsync(DevicePairingProtectionLevel::Encryption)
            .map_err(platform)?
            .await
            .map_err(platform)?;
        Ok(outcome(result.Status().map_err(platform)?))
    }

    pub(super) fn outcome(status: S) -> PairingOutcome {
        match status {
            S::Paired => PairingOutcome::Paired,
            S::AlreadyPaired => PairingOutcome::AlreadyPaired,
            S::AuthenticationTimeout => {
                PairingOutcome::NeedsOsPairing("the code was not confirmed in time".into())
            }
            S::AuthenticationFailure | S::InvalidCeremonyData => {
                PairingOutcome::NeedsOsPairing("the pairing code was not accepted".into())
            }
            S::ProtectionLevelCouldNotBeMet => {
                PairingOutcome::NeedsOsPairing("the device cannot pair with encryption".into())
            }
            S::NotReadyToPair => {
                PairingOutcome::NeedsOsPairing("the device is not in pairing mode".into())
            }
            S::PairingCanceled => PairingOutcome::NeedsOsPairing("pairing was cancelled".into()),
            other => PairingOutcome::NeedsOsPairing(format!(
                "Windows reported pairing status {}",
                other.0
            )),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn windows_statuses_map_to_outcomes() {
            assert_eq!(outcome(S::Paired), PairingOutcome::Paired);
            assert_eq!(outcome(S::AlreadyPaired), PairingOutcome::AlreadyPaired);
            for s in [
                S::AuthenticationTimeout,
                S::AuthenticationFailure,
                S::NotReadyToPair,
                S::Failed,
            ] {
                assert!(
                    matches!(outcome(s), PairingOutcome::NeedsOsPairing(_)),
                    "{s:?}"
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{BtAddress, PairingError, PairingOutcome};

    fn platform(e: bluer::Error) -> PairingError {
        PairingError::Platform(e.to_string())
    }

    pub(super) async fn pair(address: BtAddress) -> Result<PairingOutcome, PairingError> {
        let session = bluer::Session::new().await.map_err(platform)?;
        let adapter = session.default_adapter().await.map_err(platform)?;
        let device = adapter
            .device(bluer::Address(address.0))
            .map_err(platform)?;
        if device.is_paired().await.map_err(platform)? {
            return Ok(PairingOutcome::AlreadyPaired);
        }
        match device.pair().await {
            Ok(()) => Ok(PairingOutcome::Paired),
            // No agent for passkey entry, timeout, rejected: guide the crew member.
            Err(e) => Ok(PairingOutcome::NeedsOsPairing(e.to_string())),
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{BtAddress, PairingError, PairingOutcome};

    pub(super) async fn pair(_address: BtAddress) -> Result<PairingOutcome, PairingError> {
        Ok(PairingOutcome::OnFirstAccess)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_format_and_convert() {
        let a = BtAddress::parse("a4:c1:38:0B:2D:FF").expect("address");
        assert_eq!(a.to_string(), "A4:C1:38:0B:2D:FF");
        assert_eq!(a.as_u64(), 0xA4C1_380B_2DFF);
        for bad in [
            "",
            "A4:C1:38:0B:2D",
            "A4:C1:38:0B:2D:FF:00",
            "A4-C1-38-0B-2D-FF",
            "G4:C1:38:0B:2D:FF",
            "A:C1:38:0B:2D:FF0",
        ] {
            assert_eq!(BtAddress::parse(bad), None, "{bad}");
        }
    }

    /// F-ACQ-01: the guided fallback always has steps; Windows and macOS link their settings page.
    #[test]
    fn f_acq_01_os_pairing_guide_exists_on_every_platform() {
        let guide = os_pairing_guide();
        assert!(guide.steps.len() >= 3);
        assert_eq!(
            guide.settings_uri.is_some(),
            cfg!(any(windows, target_os = "macos"))
        );
    }
}
