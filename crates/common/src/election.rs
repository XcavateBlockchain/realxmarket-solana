/// The standing after a plurality count: who leads, with how much, whether
/// anything else matched it, and the total for the quorum check.
pub struct Tally {
    pub total: u64,
    /// Index of the leading entry; `None` for an empty field.
    pub leader: Option<usize>,
    pub top_power: u64,
    pub tied: bool,
}

/// Count a round by plurality. A later entry matching the current best marks
/// a tie; a later entry beating it clears one. Returns `None` only when the
/// total overflows, which callers map onto their overflow error.
pub fn tally_plurality(powers: impl IntoIterator<Item = u64>) -> Option<Tally> {
    let mut total: u64 = 0;
    let mut leader = None;
    let mut top_power: u64 = 0;
    let mut tied = false;
    for (index, power) in powers.into_iter().enumerate() {
        total = total.checked_add(power)?;
        if leader.is_some() && power == top_power {
            tied = true;
        } else if leader.is_none() || power > top_power {
            tied = false;
            leader = Some(index);
            top_power = power;
        }
    }
    Some(Tally {
        total,
        leader,
        top_power,
        tied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(powers: &[u64]) -> Tally {
        tally_plurality(powers.iter().copied()).unwrap()
    }

    #[test]
    fn a_late_leader_clears_an_earlier_tie() {
        let tally = t(&[5, 5, 7]);
        assert_eq!(
            (tally.leader, tally.top_power, tally.tied),
            (Some(2), 7, false)
        );
        assert_eq!(tally.total, 17);
    }

    #[test]
    fn an_early_leader_survives_lower_entries() {
        let tally = t(&[7, 5, 5]);
        assert_eq!(
            (tally.leader, tally.top_power, tally.tied),
            (Some(0), 7, false)
        );
    }

    #[test]
    fn a_matched_leader_is_a_tie() {
        let tally = t(&[5, 7, 7]);
        assert!(tally.tied);
        let tally = t(&[7, 7, 5]);
        assert!(tally.tied);
    }

    #[test]
    fn zero_powers_tie_at_zero() {
        let tally = t(&[0, 0]);
        assert_eq!(
            (tally.leader, tally.top_power, tally.tied),
            (Some(0), 0, true)
        );
    }

    #[test]
    fn an_empty_field_has_no_leader() {
        let tally = t(&[]);
        assert_eq!(
            (tally.leader, tally.top_power, tally.tied),
            (None, 0, false)
        );
        assert_eq!(tally.total, 0);
    }

    #[test]
    fn an_overflowing_total_refuses() {
        assert!(tally_plurality([u64::MAX, 1]).is_none());
    }
}
