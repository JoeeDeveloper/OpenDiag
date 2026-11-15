//! BMW diagnostic addresses and names.
//!
//! **NOT VERIFIED.** This is our own short list, written from general
//! community knowledge of F-series layouts, with no public specification
//! behind it. Names are labels for display only: the scan finds ECUs by
//! asking, not by trusting this table, and a wrong or missing name never
//! changes what is sent. Extend it from your own car's scan results.
//!
//! The table is deliberately small and is not derived from any BMW data
//! file (see `docs/legal.md`).

/// `(diagnostic address, short name, description)`.
pub const KNOWN_ECUS: &[(u8, &str, &str)] = &[
    (0x10, "ZGW", "central gateway"),
    (0x12, "DME", "engine control"),
    (0x18, "EGS", "transmission control"),
    (0x29, "DSC", "stability control / ABS"),
    (0x40, "CAS", "access / start system"),
    (0x60, "KOMBI", "instrument cluster"),
];

/// Short name for an address, if it is in the table.
pub fn name(address: u8) -> Option<&'static str> {
    KNOWN_ECUS
        .iter()
        .find(|(a, _, _)| *a == address)
        .map(|(_, n, _)| *n)
}

/// Description for an address, if it is in the table.
pub fn description(address: u8) -> Option<&'static str> {
    KNOWN_ECUS
        .iter()
        .find(|(a, _, _)| *a == address)
        .map(|(_, _, d)| *d)
}

/// The addresses a default scan probes.
pub fn known_addresses() -> Vec<u8> {
    KNOWN_ECUS.iter().map(|(a, _, _)| *a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_unique_and_lookup_works() {
        let mut a = known_addresses();
        a.sort_unstable();
        a.dedup();
        assert_eq!(a.len(), KNOWN_ECUS.len());
        assert_eq!(name(0x12), Some("DME"));
        assert_eq!(name(0xEE), None);
        assert_eq!(description(0x10), Some("central gateway"));
    }
}
