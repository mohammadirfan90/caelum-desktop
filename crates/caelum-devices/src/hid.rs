//! USB / serial adapter framework, HID part (F-ACQ-04): device discovery and read-only access to
//! input reports. Per-device decoders work on the reports.
//!
//! A device is handed out only as [`ReadOnlyHid`], which can read input reports and nothing else:
//! output and feature reports (writes) are not reachable from adapters. Any HID write a device ICD
//! needs would be a write exception (`docs/write-exceptions.md`).

use std::time::Duration;

/// A discovered HID device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidInfo {
    /// Platform path (opens exactly this interface).
    pub path: String,
    /// USB vendor ID.
    pub vendor_id: u16,
    /// USB product ID.
    pub product_id: u16,
    /// Serial number, when reported.
    pub serial_number: Option<String>,
    /// Product string, when reported.
    pub product: Option<String>,
    /// HID usage page of the interface.
    pub usage_page: u16,
}

/// HID errors.
#[derive(Debug, thiserror::Error)]
pub enum HidError {
    /// The platform HID layer failed.
    #[error("HID: {0}")]
    Platform(String),
    /// The path contains a NUL byte.
    #[error("invalid HID path")]
    InvalidPath,
}

impl From<hidapi::HidError> for HidError {
    fn from(e: hidapi::HidError) -> Self {
        HidError::Platform(e.to_string())
    }
}

/// The HID layer of the OS.
pub struct HidHost(hidapi::HidApi);

impl HidHost {
    /// Opens the platform HID layer.
    pub fn open() -> Result<Self, HidError> {
        Ok(Self(hidapi::HidApi::new()?))
    }

    /// Devices currently attached, optionally only one vendor/product.
    pub fn devices(&mut self, filter: Option<(u16, u16)>) -> Result<Vec<HidInfo>, HidError> {
        self.0.refresh_devices()?;
        Ok(self
            .0
            .device_list()
            .filter(|d| filter.is_none_or(|(v, p)| d.vendor_id() == v && d.product_id() == p))
            .map(|d| HidInfo {
                path: d.path().to_string_lossy().into_owned(),
                vendor_id: d.vendor_id(),
                product_id: d.product_id(),
                serial_number: d.serial_number().map(str::to_owned),
                product: d.product_string().map(str::to_owned),
                usage_page: d.usage_page(),
            })
            .collect())
    }

    /// Opens one interface for reading input reports.
    pub fn open_read_only(&self, path: &str) -> Result<ReadOnlyHid, HidError> {
        let c = std::ffi::CString::new(path).map_err(|_| HidError::InvalidPath)?;
        Ok(ReadOnlyHid(self.0.open_path(&c)?))
    }
}

/// A HID interface that can only deliver input reports.
pub struct ReadOnlyHid(hidapi::HidDevice);

impl ReadOnlyHid {
    /// Waits up to `timeout` for one input report; `None` if none arrived. A report longer than
    /// `max_len` is truncated by the platform, so `max_len` comes from the device's report
    /// descriptor or ICD.
    pub fn next_report(
        &self,
        max_len: usize,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, HidError> {
        let mut buf = vec![0u8; max_len];
        let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let n = self.0.read_timeout(&mut buf, ms)?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }
}
