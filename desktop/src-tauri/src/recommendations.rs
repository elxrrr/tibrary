//! Pure recommendation policy. Evidence extraction stays at the database boundary.

use serde_json::Value;
use std::collections::HashSet;

pub fn credit_names(credits: &Value) -> HashSet<String> {
    credits
        .as_array()
        .into_iter()
        .flatten()
        .filter(|credit| {
            ["role", "roleId"].iter().any(|field| {
                credit[field]
                    .as_str()
                    .is_some_and(|role| !role.trim().is_empty())
            })
        })
        .filter_map(|credit| {
            credit["name"]
                .as_str()
                .or_else(|| credit["person"].as_str())
        })
        .map(crate::matching::name_key)
        .filter(|name| !name.is_empty())
        .collect()
}

pub fn local_credit_names(metadata: &Value) -> HashSet<String> {
    let mut names = HashSet::new();
    for role in [
        "composer",
        "lyricist",
        "songwriter",
        "producer",
        "engineer",
        "mixer",
    ] {
        let value = metadata["tags"]
            .get(role)
            .or_else(|| metadata.get(role))
            .unwrap_or(&Value::Null);
        let values: Vec<&str> = if let Some(values) = value.as_array() {
            values.iter().filter_map(Value::as_str).collect()
        } else {
            value.as_str().into_iter().collect()
        };
        for value in values {
            names.extend(
                value
                    .split(';')
                    .map(crate::matching::name_key)
                    .filter(|name| !name.is_empty()),
            );
        }
    }
    names
}

/// Count independently corroborated contributors from already indexed music.
/// A lone shared name never makes a release "Recommended".
pub fn shared_contributor_evidence(
    local: Option<&HashSet<String>>,
    tracks: Option<&Vec<Value>>,
) -> (usize, usize) {
    let (Some(local), Some(tracks)) = (local, tracks) else {
        return (0, 0);
    };
    let mut people = HashSet::new();
    let mut matched_tracks = 0;
    let mut seen_recordings = HashSet::new();
    for track in tracks {
        let names = credit_names(&track["credits"]);
        let shared: Vec<_> = names.intersection(local).cloned().collect();
        if !shared.is_empty() {
            let recording = track["isrc"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| track["id"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| track.to_string());
            if !seen_recordings.insert(recording) {
                continue;
            }
            people.extend(shared);
            matched_tracks += 1;
        }
    }
    (matched_tracks, people.len())
}

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
    fn credits_need_two_local_anchors_for_strong_network() {
        let local = HashSet::from(["writerone".to_string(), "writertwo".to_string()]);
        let tracks = vec![
            serde_json::json!({"credits":[{"roleId":"songwriter","name":"Writer One"}]}),
            serde_json::json!({"credits":[{"roleId":"producer","name":"Writer Two"}]}),
        ];
        assert_eq!(
            shared_contributor_evidence(Some(&local), Some(&tracks)),
            (2, 2)
        );
        assert_eq!(
            shared_contributor_evidence(Some(&local), Some(&tracks[..1].to_vec())),
            (1, 1)
        );
    }

    #[test]
    fn duplicate_editions_and_missing_roles_do_not_inflate_credit_evidence() {
        let local = HashSet::from(["writer".into()]);
        let track =
            serde_json::json!({"isrc":"GB123","credits":[{"name":"Writer","role":"Composer"}]});
        let tracks = vec![
            track.clone(),
            track,
            serde_json::json!({"id":"other","credits":[{"name":"Writer","roleId":null}]}),
        ];
        assert_eq!(
            shared_contributor_evidence(Some(&local), Some(&tracks)),
            (1, 1)
        );
    }

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
