/// What decides which copy of an item wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub updated_at: u64,
    pub deleted: bool,
    pub device: String,
}

/// Is `a` strictly newer than `b`? Newer `updated_at` wins; ties go to the larger device id;
/// a full tie is not "newer", so both sides keep what they have.
pub fn newer(a: &Stamp, b: &Stamp) -> bool {
    (a.updated_at, &a.device) > (b.updated_at, &b.device)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(t: u64, d: bool, dev: &str) -> Stamp {
        Stamp { updated_at: t, deleted: d, device: dev.into() }
    }
    #[test]
    fn newer_wins() {
        assert!(newer(&s(2, false, "a"), &s(1, false, "z")));
        assert!(!newer(&s(1, false, "z"), &s(2, false, "a")));
    }
    #[test]
    fn tie_broken_by_device() {
        assert!(newer(&s(5, false, "b"), &s(5, false, "a")));
        assert!(!newer(&s(5, false, "a"), &s(5, false, "b")));
    }
    #[test]
    fn full_tie_is_not_newer() {
        assert!(!newer(&s(5, false, "a"), &s(5, true, "a")));
        assert!(!newer(&s(5, true, "a"), &s(5, false, "a")));
    }
    #[test]
    fn tombstone_newer_beats_edit() {
        assert!(newer(&s(9, true, "a"), &s(3, false, "b")));
    }
    #[test]
    fn edit_newer_beats_tombstone() {
        assert!(newer(&s(9, false, "a"), &s(3, true, "b")));
    }
    #[test]
    fn merge_is_commutative() {
        let mut r = [0u8; 8];
        for _ in 0..1000 {
            getrandom::fill(&mut r).unwrap();
            let a = s((r[0] % 4) as u64, r[1] % 2 == 0, if r[2] % 2 == 0 { "a" } else { "b" });
            let b = s((r[3] % 4) as u64, r[4] % 2 == 0, if r[5] % 2 == 0 { "a" } else { "b" });
            let w1 = if newer(&a, &b) { &a } else { &b };
            let w2 = if newer(&b, &a) { &b } else { &a };
            assert_eq!((w1.updated_at, &w1.device), (w2.updated_at, &w2.device));
        }
    }
}
