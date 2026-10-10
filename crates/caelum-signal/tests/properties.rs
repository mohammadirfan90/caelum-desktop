//! Property tests (F-SIG-01, F-SIG-02): arbitrary finite input never panics, QRS events are
//! strictly increasing and inside the record, SQI scores stay in [0, 1], and chunking does not
//! change the detector's output (streaming = batch).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_signal::ecg::{detect_qrs, QrsDetector};
use caelum_signal::sqi::ecg_sqi;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn f_sig_01_qrs_events_are_ordered_and_in_range(
        x in prop::collection::vec(-5.0f64..5.0, 0..3_000),
        fs in prop::sample::select(vec![100.0, 250.0, 360.0, 500.0]),
    ) {
        let ev = detect_qrs(&x, fs).unwrap();
        for w in ev.windows(2) {
            prop_assert!(w[1].sample > w[0].sample);
        }
        prop_assert!(ev.iter().all(|e| (e.sample as usize) < x.len()));
    }

    #[test]
    fn f_sig_01_chunking_does_not_change_detection(
        x in prop::collection::vec(-2.0f64..2.0, 500..2_500),
        chunk in 1usize..400,
    ) {
        let mut a = QrsDetector::new(250.0).unwrap();
        let whole = a.push(&x).unwrap();
        let mut b = QrsDetector::new(250.0).unwrap();
        let mut parts = Vec::new();
        for c in x.chunks(chunk) {
            parts.extend(b.push(c).unwrap());
        }
        prop_assert_eq!(whole, parts);
    }

    #[test]
    fn f_sig_02_sqi_is_bounded(
        x in prop::collection::vec(-10.0f64..10.0, 500..2_000),
        beats in prop::collection::vec(0usize..2_000, 0..20),
    ) {
        let beats: Vec<usize> = beats.into_iter().filter(|b| *b < x.len()).collect();
        let r = ecg_sqi(&x, 250.0, &beats).unwrap();
        prop_assert!((0.0..=1.0).contains(&r.score));
    }
}
