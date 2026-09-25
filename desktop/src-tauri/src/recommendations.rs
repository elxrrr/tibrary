//! Pure recommendation policy. Evidence extraction stays at the database boundary.

#[allow(clippy::too_many_arguments)]
pub fn recommendation_score(
    primary: bool,
    conflict: bool,
    catalogue: bool,
    label: bool,
    label_supported: bool,
    rights: bool,
    recordings: usize,
    contributor_tracks: usize,
    contributors: usize,
    compilation: bool,
    unofficial: bool,
    official: bool,
) -> (i32, String, Vec<String>) {
    let mut score = 30;
    let mut reasons = Vec::new();
    if primary {
        score += 30;
        reasons.push("Verified album artist credit matches a linked artist".into());
    } else if conflict {
        score -= 25;
        reasons.push("Album artist credits differ from the linked artist".into());
    } else {
        reasons.push(
            "Primary release credits are not cached; catalogue placement alone is insufficient"
                .into(),
        );
    }
    if catalogue {
        score += 5;
        reasons.push("Listed in a linked artist catalogue".into());
    }
    if label {
        score += 20;
        reasons.push("Record label matches the local discography".into());
    }
    if rights {
        score += 20;
        reasons.push("Copyright holder matches the local discography".into());
    }
    if recordings > 0 {
        score += 10;
        reasons.push(format!(
            "{recordings} recording identifiers occur in the local discography"
        ));
    }
    // Count distinct people and distinct recordings, not duplicated credit roles/editions.
    let network = contributor_tracks >= 2 && contributors >= 2;
    if contributors > 0 {
        score += if network { 20 } else { 5 };
        reasons.push(format!("{contributors} shared contributors across {contributor_tracks} recordings, anchored to linked local music"));
    }
    if compilation {
        score -= 20;
        reasons.push("Compilation; inspect artist roles before choosing".into());
    }
    if unofficial {
        score -= 35;
        reasons.push("Provider marks the release unofficial".into());
    } else if official {
        score += 5;
        reasons.push("Provider marks the release official".into());
    }
    let strong = primary
        && (label_supported || rights || network || recordings >= 2)
        && !compilation
        && !unofficial;
    if strong && (network || recordings >= 2) {
        score = score.max(85);
    }
    if !strong {
        score = score.min(79);
    }
    score = score.clamp(0, 100);
    let badge = if score < 20 {
        "Unmatched"
    } else if strong && score >= 85 {
        "Recommended"
    } else if conflict || unofficial || compilation {
        "Suspect"
    } else {
        "Potential"
    };
    (score, badge.into(), reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contributor_network_requires_primary_and_independent_recordings() {
        let score = |primary, conflict, tracks, people| {
            recommendation_score(
                primary, conflict, true, false, false, false, 0, tracks, people, false, false,
                false,
            )
            .1
        };
        assert_eq!(score(true, false, 2, 2), "Recommended");
        assert_eq!(score(true, false, 1, 2), "Potential");
        assert_eq!(score(false, true, 2, 2), "Suspect");
        assert_eq!(score(false, false, 2, 2), "Potential");
    }
}
