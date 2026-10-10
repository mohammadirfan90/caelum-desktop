//! ⚕ DRAFT solar-particle-event states (F-ENV-02, ICD-VEH OI-2, CR-015 §4).
//!
//! These are CAELUM's state names for the `spe_state` parameter. The vehicle's integer codes for
//! them come from the vehicle mission database through the signed MID table; a code the table does
//! not map is rejected, never guessed. The catalog entry in `caelum-domain` changes from
//! `EnumPending` to these states when CR-015 is integrated (docs/integration/M9.md §4).

/// The states, in order of increasing severity except `all_clear`.
///
/// - `none`: no event in progress and none forecast;
/// - `watch`: an event is forecast or conditions favour one;
/// - `warning`: an event is imminent or proton flux is rising toward the event threshold;
/// - `in_progress`: an event is in progress (shelter procedures apply);
/// - `all_clear`: an event has ended and the all-clear has been given.
pub const SPE_STATES: [&str; 5] = ["none", "watch", "warning", "in_progress", "all_clear"];

#[cfg(test)]
mod tests {
    use super::*;

    /// F-ENV-02 / CR-015 §4: the vocabulary is fixed and distinct.
    #[test]
    fn f_env_02_spe_states_are_distinct_snake_case() {
        let mut s = SPE_STATES.to_vec();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), SPE_STATES.len());
        assert!(SPE_STATES
            .iter()
            .all(|n| n.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')));
    }
}
