//! Bluetooth SIG health-profile emulators: they encode the values a test scripts into the exact
//! payloads real devices send, so decoders, ingest and storage are exercised end to end. Values
//! always come from the test or scenario; emulators never invent one.

/// Characteristic UUIDs (same as `caelum_devices::sig`).
pub mod uuid {
    /// Heart Rate Measurement.
    pub const HEART_RATE_MEASUREMENT: u16 = 0x2A37;
    /// PLX Spot-Check Measurement.
    pub const PLX_SPOT_CHECK: u16 = 0x2A5E;
    /// PLX Continuous Measurement.
    pub const PLX_CONTINUOUS: u16 = 0x2A5F;
    /// Blood Pressure Measurement.
    pub const BLOOD_PRESSURE_MEASUREMENT: u16 = 0x2A35;
    /// Temperature Measurement.
    pub const TEMPERATURE_MEASUREMENT: u16 = 0x2A1C;
    /// Weight Measurement.
    pub const WEIGHT_MEASUREMENT: u16 = 0x2A9D;
    /// Glucose Measurement.
    pub const GLUCOSE_MEASUREMENT: u16 = 0x2A18;
}

/// IEEE 11073 SFLOAT from a mantissa (−2048..=2047, excluding the special codes) and exponent
/// (−8..=7).
pub fn sfloat(mantissa: i16, exponent: i8) -> u16 {
    let m = (mantissa as u16) & 0x0FFF;
    let e = ((exponent as u16) & 0x0F) << 12;
    e | m
}

/// IEEE 11073 FLOAT from a mantissa (24-bit signed) and exponent.
pub fn float(mantissa: i32, exponent: i8) -> u32 {
    ((exponent as u8 as u32) << 24) | ((mantissa as u32) & 0x00FF_FFFF)
}

/// SFLOAT NaN (a device that has no value).
pub const SFLOAT_NAN: u16 = 0x07FF;

/// A chest strap: heart rate (UINT8) with skin contact and RR intervals (1/1024 s).
pub fn heart_rate(bpm: u8, contact: Option<bool>, rr_1024: &[u16]) -> Vec<u8> {
    let mut flags = 0u8;
    match contact {
        Some(true) => flags |= 0x06,
        Some(false) => flags |= 0x04,
        None => {}
    }
    if !rr_1024.is_empty() {
        flags |= 0x10;
    }
    let mut b = vec![flags, bpm];
    for rr in rr_1024 {
        b.extend(rr.to_le_bytes());
    }
    b
}

/// A continuous pulse oximeter with Measurement Status.
pub fn plx_continuous(spo2: u16, pulse: u16, measurement_status: u16) -> Vec<u8> {
    let mut b = vec![0x04];
    b.extend(spo2.to_le_bytes());
    b.extend(pulse.to_le_bytes());
    b.extend(measurement_status.to_le_bytes());
    b
}

/// A spot-check pulse oximeter with Measurement Status (no timestamp).
pub fn plx_spot_check(spo2: u16, pulse: u16, measurement_status: u16) -> Vec<u8> {
    let mut b = vec![0x02];
    b.extend(spo2.to_le_bytes());
    b.extend(pulse.to_le_bytes());
    b.extend(measurement_status.to_le_bytes());
    b
}

/// A blood-pressure cuff in mmHg with pulse rate and a user slot.
pub fn blood_pressure(systolic: u16, diastolic: u16, map: u16, pulse: u16, user: u8) -> Vec<u8> {
    let mut b = vec![0x0C];
    // Field order: systolic, diastolic, mean arterial pressure, pulse rate.
    for v in [systolic, diastolic, map, pulse] {
        b.extend(v.to_le_bytes());
    }
    b.push(user);
    b
}

/// A thermometer in °C with an optional HTS Temperature Type (site code).
pub fn temperature(celsius: u32, site: Option<u8>) -> Vec<u8> {
    let mut b = vec![if site.is_some() { 0x04 } else { 0x00 }];
    b.extend(celsius.to_le_bytes());
    if let Some(s) = site {
        b.push(s);
    }
    b
}

/// A scale in kg (resolution 0.005 kg) with a user slot.
pub fn weight(raw_0_005_kg: u16, user: u8) -> Vec<u8> {
    let mut b = vec![0x04];
    b.extend(raw_0_005_kg.to_le_bytes());
    b.push(user);
    b
}

/// A glucose meter in kg/L (SFLOAT) with sample type/location and base time 2017-01-01.
pub fn glucose(sequence: u16, kg_per_l: u16, type_location: u8) -> Vec<u8> {
    let mut b = vec![0x02];
    b.extend(sequence.to_le_bytes());
    b.extend([0xE1, 0x07, 1, 1, 0, 0, 0]);
    b.extend(kg_per_l.to_le_bytes());
    b.push(type_location);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfloat_and_float_encode_mantissa_and_exponent() {
        assert_eq!(sfloat(975, -1), 0xF3CF);
        assert_eq!(sfloat(72, 0), 0x0048);
        assert_eq!(float(370, -1), 0xFF00_0172);
    }

    #[test]
    fn payload_layouts() {
        assert_eq!(heart_rate(72, Some(true), &[1024]), [0x16, 72, 0x00, 0x04]);
        assert_eq!(temperature(float(370, -1), Some(3)).len(), 6);
        assert_eq!(
            blood_pressure(120, 80, 93, 70, 1),
            [0x0C, 120, 0, 80, 0, 93, 0, 70, 0, 1]
        );
    }
}
