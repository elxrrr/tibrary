use crate::matching::title_key;
use crate::tidal::{TidalRelease, TidalTrack};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTrackInfo {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub track_number: u32,
    pub disc_number: u32,
    pub isrc: Option<String>,
}

pub fn clean_isrc(isrc_opt: Option<&str>) -> Option<String> {
    isrc_opt.map(|s| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase()
    })
}

pub fn has_mix_keyword(title: &str) -> bool {
    let lower = title.to_lowercase();
    let keywords = [
        "remix",
        "mix",
        "extended",
        "instrumental",
        "radio",
        "club",
        "live",
        "edit",
        "acoustic",
    ];
    keywords
        .iter()
        .any(|&kw| lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == kw))
}

/// Credit-name equivalence for a release that has already been established by
/// recording and structural evidence. Shared names alone never establish an
/// alias: both names must belong to the same unambiguous cached identity.
#[derive(Debug, Default)]
pub struct CreditAliases {
    identity_names: HashMap<String, HashSet<String>>,
    name_identities: HashMap<String, HashSet<String>>,
    bridged_ids: HashSet<String>,
    batching: bool,
    dirty: bool,
}

impl CreditAliases {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_batch(&mut self) {
        self.batching = true;
    }

    pub fn finish_batch(&mut self) {
        self.batching = false;
        self.extend_releases(std::iter::empty());
    }

    pub fn extend_releases<'a>(&mut self, releases: impl IntoIterator<Item = &'a TidalRelease>) {
        let mut changed = false;
        for track in releases.into_iter().flat_map(|release| &release.tracks) {
            for (identity, name) in credited_names(track) {
                if let Some(identity) = identity {
                    let identity = self.identity_key(&identity);
                    changed |= self
                        .identity_names
                        .entry(identity.clone())
                        .or_default()
                        .insert(name.clone());
                }
            }
        }
        self.dirty |= changed;
        if self.batching || !self.dirty {
            return;
        }
        // Subscriber contributor IDs may also identify artists. Bridge these
        // namespaces only when both the numeric ID and an observed name agree.
        let bridges: Vec<_> = self
            .identity_names
            .iter()
            .filter_map(|(identity, names)| {
                let id = identity.strip_prefix("artist:")?;
                if id.parse::<u64>().ok().filter(|id| *id > 0).is_none() {
                    return None;
                }
                self.identity_names
                    .get(&format!("contributor:{id}"))
                    .filter(|other| !names.is_disjoint(other))
                    .map(|_| id.to_owned())
            })
            .collect();
        for id in bridges {
            if let Some(names) = self.identity_names.remove(&format!("contributor:{id}")) {
                self.identity_names
                    .entry(format!("artist:{id}"))
                    .or_default()
                    .extend(names);
                self.bridged_ids.insert(id);
            }
        }
        self.name_identities.clear();
        for (identity, names) in &self.identity_names {
            for name in names {
                self.name_identities
                    .entry(name.clone())
                    .or_default()
                    .insert(identity.clone());
            }
        }
        self.dirty = false;
    }

    /// Callers independently establish recording/release context, a positive
    /// duration within three seconds and matching positions/totals. Omitted
    /// remix annotations additionally require an equal ISRC. This helper never
    /// establishes a release or changes the global strict recording matcher.
    pub fn titles_match(
        &self,
        local_title: &str,
        remote: &TidalTrack,
        allow_unnamed_remix: bool,
    ) -> bool {
        let local_key = title_key(local_title);
        if !local_key.is_empty() && local_key == title_key(&remote.title) {
            return true;
        }
        let credits = credited_names(remote);
        let identities: HashSet<_> = credits
            .iter()
            .filter_map(|(id, _)| id.as_deref())
            .map(|id| self.identity_key(id))
            .collect();
        let mut explicit_names = HashMap::<String, HashSet<String>>::new();
        for (identity, name) in &credits {
            if let Some(identity) = identity {
                explicit_names
                    .entry(name.clone())
                    .or_default()
                    .insert(self.identity_key(identity));
            }
        }
        let remixer_ids: HashSet<_> = remote
            .credits
            .as_array()
            .into_iter()
            .flatten()
            .filter(|credit| {
                credit["role"]
                    .as_str()
                    .is_some_and(|role| role.eq_ignore_ascii_case("Remixer"))
            })
            .filter_map(credit_identity)
            .map(|id| self.identity_key(&id))
            .collect();
        // Writing/production credits can survive on a different performance.
        // They do not prove that the locally named featured artist is audible.
        let performers = credited_names_for_role_scope(remote, true);
        let performer_identities: HashSet<_> = performers
            .iter()
            .filter_map(|(id, _)| id.as_deref())
            .map(|id| self.identity_key(id))
            .collect();
        let mut known_names: HashSet<_> = performers.iter().map(|(_, name)| name.clone()).collect();
        for identity in &performer_identities {
            for name in self.identity_names.get(identity).into_iter().flatten() {
                if self
                    .name_identities
                    .get(name)
                    .is_some_and(|ids| ids.len() == 1)
                {
                    known_names.insert(name.clone());
                }
            }
        }
        let local = strip_known_features(local_title, &known_names);
        let remote = strip_known_features(&remote.title, &known_names);
        if !title_key(&local).is_empty() && title_key(&local) == title_key(&remote) {
            return true;
        }
        if allow_unnamed_remix
            && credited_mix_annotation_matches(&local, &remote, &explicit_names, &remixer_ids)
        {
            return true;
        }
        let (local_base, local_mix) = split_mix_annotation(&local);
        let (remote_base, remote_mix) = split_mix_annotation(&remote);
        if local_base.is_empty() || local_base != remote_base {
            return false;
        }
        let (Some(local_mix), Some(remote_mix)) = (local_mix, remote_mix) else {
            return false;
        };
        if self.canonical_mix(&local_mix, &identities, &explicit_names)
            == self.canonical_mix(&remote_mix, &identities, &explicit_names)
        {
            return true;
        }
        allow_unnamed_remix
            && local_mix == "remix"
            && remote_mix.ends_with(" remix")
            && remote_mix
                .split_whitespace()
                .filter(|word| has_mix_keyword(word))
                .eq(["remix"])
    }

    fn identity_key(&self, identity: &str) -> String {
        if let Some(id) = identity.strip_prefix("contributor:") {
            if self.bridged_ids.contains(id) {
                return format!("artist:{id}");
            }
        }
        identity.to_owned()
    }

    fn canonical_mix(
        &self,
        mix: &str,
        identities: &HashSet<String>,
        explicit_names: &HashMap<String, HashSet<String>>,
    ) -> String {
        let mut names: Vec<_> = self
            .name_identities
            .iter()
            .filter_map(|(name, ids)| {
                let id = if ids.len() == 1 {
                    ids.iter().next()?
                } else {
                    // An unrelated artist can share this alias globally. The
                    // compared track itself must name one exact person; merely
                    // intersecting its contributors with the cache is unsafe.
                    explicit_names
                        .get(name)
                        .filter(|ids| ids.len() == 1)?
                        .iter()
                        .next()?
                };
                identities
                    .contains(id.as_str())
                    .then_some((name.as_str(), id.as_str()))
            })
            .collect();
        names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        let mut remainder = mix;
        let mut result = String::new();
        while !remainder.is_empty() {
            if let Some((name, id)) = names.iter().find(|(name, _)| {
                remainder.starts_with(name) && remainder[name.len()..].starts_with(' ')
            }) {
                result.push_str(&format!("[{id}]"));
                remainder = &remainder[name.len()..];
            } else {
                let word_end = remainder.find(' ').unwrap_or(remainder.len());
                result.push_str(&remainder[..word_end]);
                remainder = &remainder[word_end..];
            }
            if remainder.starts_with(' ') {
                result.push(' ');
                remainder = &remainder[1..];
            }
        }
        result
    }
}

fn credited_names(track: &TidalTrack) -> Vec<(Option<String>, String)> {
    credited_names_for_role_scope(track, false)
}

fn credited_names_for_role_scope(
    track: &TidalTrack,
    performers_only: bool,
) -> Vec<(Option<String>, String)> {
    let mut names = Vec::new();
    for artist in &track.artists {
        if let Some(name) = artist["name"]
            .as_str()
            .map(title_key)
            .filter(|name| !name.is_empty())
        {
            names.push((
                person_id(&artist["id"]).map(|id| format!("artist:{id}")),
                name,
            ));
        }
    }
    for credit in track.credits.as_array().into_iter().flatten() {
        if performers_only && !credit["role"].as_str().is_some_and(is_performance_role) {
            continue;
        }
        let Some(name) = credit["name"]
            .as_str()
            .map(title_key)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        // A generic credit resource ID identifies a credit, not its person.
        names.push((credit_identity(credit), name));
    }
    names
}

fn is_performance_role(role: &str) -> bool {
    matches!(
        title_key(role).as_str(),
        "artist"
            | "main artist"
            | "primary artist"
            | "featured artist"
            | "performer"
            | "featured performer"
            | "guest performer"
            | "vocal"
            | "vocals"
            | "vocalist"
            | "singer"
            | "lead vocal"
            | "lead vocals"
            | "lead vocalist"
            | "backing vocals"
            | "backing vocalist"
            | "background vocals"
            | "background vocalist"
            | "additional vocals"
            | "instrumentalist"
            | "guitar"
            | "guitarist"
            | "bass"
            | "bassist"
            | "drums"
            | "drummer"
            | "piano"
            | "pianist"
            | "violin"
            | "violinist"
            | "saxophone"
            | "saxophonist"
            | "trumpet"
            | "trumpeter"
            | "keyboard"
            | "keyboards"
    )
}

fn person_id(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|id| !id.trim().is_empty() && *id != "0")
        .map(str::to_owned)
        .or_else(|| value.as_u64().filter(|id| *id > 0).map(|id| id.to_string()))
}

fn credit_identity(credit: &serde_json::Value) -> Option<String> {
    let artist_id = ["artist_id", "artistId"]
        .iter()
        .find_map(|key| person_id(&credit[*key]))
        .or_else(|| person_id(&credit["relationships"]["artist"]["data"]["id"]));
    artist_id.map(|id| format!("artist:{id}")).or_else(|| {
        ["contributor_id", "contributorId", "person_id", "personId"]
            .iter()
            .find_map(|key| person_id(&credit[*key]))
            .map(|id| format!("contributor:{id}"))
    })
}

/// A trailing list of explicitly credited people can omit the generic word
/// "Mix". This applies only inside the caller's equal-ISRC release context and
/// requires a stable, explicitly credited remixer among the exact same people.
fn credited_mix_annotation_matches(
    local: &str,
    remote: &str,
    explicit: &HashMap<String, HashSet<String>>,
    remixers: &HashSet<String>,
) -> bool {
    fn terminal(title: &str) -> Option<(&str, &str)> {
        let title = title.trim();
        for (open, close) in [('(', ')'), ('[', ']')] {
            if title.ends_with(close) {
                let start = title.rfind(open)?;
                return Some((&title[..start], &title[start + 1..title.len() - 1]));
            }
        }
        None
    }
    fn people(
        annotation: &str,
        explicit: &HashMap<String, HashSet<String>>,
    ) -> Option<HashSet<String>> {
        let lookup = |name: &str| {
            explicit
                .get(&title_key(name))
                .filter(|ids| ids.len() == 1)
                .and_then(|ids| ids.iter().next())
                .cloned()
        };
        if let Some(id) = lookup(annotation) {
            return Some(HashSet::from([id]));
        }
        let annotation = annotation.replace(" and ", " & ");
        annotation.split([',', '&']).map(lookup).collect()
    }
    let (Some((local_base, local_people)), Some((remote_base, remote_mix))) =
        (terminal(local), terminal(remote))
    else {
        return false;
    };
    if title_key(local_base).is_empty()
        || title_key(local_base) != title_key(remote_base)
        || has_mix_keyword(local_people)
    {
        return false;
    }
    let remote_key = title_key(remote_mix);
    if !remote_key.ends_with(" mix")
        || !remote_key
            .split_whitespace()
            .filter(|word| has_mix_keyword(word))
            .eq(["mix"])
    {
        return false;
    }
    let Some(end) = remote_mix.trim_end().rfind(char::is_whitespace) else {
        return false;
    };
    let (Some(local_people), Some(remote_people)) = (
        people(local_people, explicit),
        people(&remote_mix[..end], explicit),
    ) else {
        return false;
    };
    !local_people.is_empty() && local_people == remote_people && !local_people.is_disjoint(remixers)
}

fn known_feature(annotation: &str, names: &HashSet<String>) -> bool {
    let annotation = annotation.trim();
    let lower = annotation.to_lowercase();
    let Some(prefix) = ["featuring ", "feat. ", "feat ", "ft. ", "ft ", "with "]
        .into_iter()
        .find(|prefix| lower.starts_with(prefix))
    else {
        return false;
    };
    let people = annotation[prefix.len()..].trim();
    if names.contains(&title_key(people)) {
        return true;
    }
    let people = people.replace(" and ", " & ");
    let parts: Vec<_> = people.split([',', '&']).map(title_key).collect();
    !parts.is_empty()
        && parts
            .iter()
            .all(|name| !name.is_empty() && names.contains(name))
}

fn strip_known_features(title: &str, names: &HashSet<String>) -> String {
    let mut result = String::new();
    let mut remainder = title;
    while let Some(start) = remainder.find(['(', '[']) {
        result.push_str(&remainder[..start]);
        let close = if remainder[start..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(end) = remainder[start + 1..]
            .find(close)
            .map(|end| start + 1 + end)
        else {
            result.push_str(&remainder[start..]);
            remainder = "";
            break;
        };
        if !known_feature(&remainder[start + 1..end], names) {
            result.push_str(&remainder[start..=end]);
        }
        remainder = &remainder[end + 1..];
    }
    result.push_str(remainder);
    for marker in [
        " featuring ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " with ",
    ] {
        // ASCII case folding preserves offsets in Unicode song titles.
        if let Some(start) = result.to_ascii_lowercase().rfind(marker) {
            if known_feature(&result[start + 1..], names) {
                result.truncate(start);
                break;
            }
        }
    }
    result.trim().to_string()
}

fn split_mix_annotation(title: &str) -> (String, Option<String>) {
    let title = title.trim();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if title.ends_with(close) {
            if let Some(start) = title.rfind(open) {
                let mix = &title[start + 1..title.len() - 1];
                if has_mix_keyword(mix) {
                    return (title_key(&title[..start]), Some(title_key(mix)));
                }
            }
        }
    }
    for separator in [" - ", " – ", " — "] {
        if let Some((base, mix)) = title.rsplit_once(separator) {
            if has_mix_keyword(mix) {
                return (title_key(base), Some(title_key(mix)));
            }
        }
    }
    (title_key(title), None)
}

pub fn recording_matches(
    local_title: &str,
    local_duration: f64,
    local_isrc: Option<&str>,
    remote_title: &str,
    remote_duration: f64,
    remote_isrc: Option<&str>,
    strict: bool,
) -> bool {
    // 1. Duration check (tolerance: 3 seconds)
    if local_duration > 0.0
        && remote_duration > 0.0
        && (local_duration - remote_duration).abs() > 3.0
    {
        return false;
    }

    // 2. ISRC check
    let li = clean_isrc(local_isrc);
    let ri = clean_isrc(remote_isrc);
    if let (Some(ref l), Some(ref r)) = (&li, &ri) {
        if !l.is_empty() && !r.is_empty() && l != r {
            return false;
        }
    }

    // 3. Mix keyword check
    let lt_key = title_key(local_title);
    let rt_key = title_key(remote_title);
    if lt_key != rt_key && (has_mix_keyword(local_title) || has_mix_keyword(remote_title)) {
        return false;
    }

    if strict {
        if local_duration <= 0.0
            || remote_duration <= 0.0
            || local_title.is_empty()
            || remote_title.is_empty()
        {
            return false;
        }
        if lt_key != rt_key {
            return false;
        }
    }

    // If both ISRCs match, that's a verified match
    if let (Some(ref l), Some(ref r)) = (&li, &ri) {
        if !l.is_empty() && !r.is_empty() && l == r {
            return true;
        }
    }

    // Otherwise, match if normalized title and duration match
    !lt_key.is_empty() && lt_key == rt_key && local_duration > 0.0 && remote_duration > 0.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackAlignment {
    pub local_path: String,
    pub remote_track_id: String,
    pub disc_number: u32,
    pub track_number: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureMatchResult {
    pub compatible: bool,
    pub incomplete: bool,
    pub matched_count: usize,
    pub total_remote_tracks: usize,
    pub conflicts: Vec<String>,
    pub alignments: HashMap<String, TrackAlignment>,
    pub missing_remote_track_ids: Vec<String>,
}

pub fn structure_match(
    local_tracks: &[LocalTrackInfo],
    remote_release: &TidalRelease,
) -> StructureMatchResult {
    let mut remote_positions = HashMap::new();
    for t in &remote_release.tracks {
        remote_positions.insert((t.disc_number, t.track_number), t);
    }

    let mut conflicts = Vec::new();
    if remote_positions.len() != remote_release.tracks.len() {
        conflicts.push("Catalogue track positions are incomplete or duplicated".to_string());
    }

    let mut claimed_positions = HashSet::new();
    let mut alignments = HashMap::new();
    let mut unmatched_local = Vec::new();

    // Pass 1: exact position matches
    for local in local_tracks {
        let pos = (local.disc_number, local.track_number);
        if let Some(remote) = remote_positions.get(&pos) {
            if recording_matches(
                &local.title,
                local.duration,
                local.isrc.as_deref(),
                &remote.title,
                remote.duration,
                remote.isrc.as_deref(),
                false,
            ) && !claimed_positions.contains(&pos)
            {
                claimed_positions.insert(pos);
                alignments.insert(
                    local.path.clone(),
                    TrackAlignment {
                        local_path: local.path.clone(),
                        remote_track_id: remote.id.clone(),
                        disc_number: pos.0,
                        track_number: pos.1,
                    },
                );
                continue;
            }
        }
        unmatched_local.push(local);
    }

    // Pass 2: unique recording match for unmatched tracks
    for local in unmatched_local {
        let mut hits = Vec::new();
        for (&pos, &remote) in &remote_positions {
            if !claimed_positions.contains(&pos)
                && recording_matches(
                    &local.title,
                    local.duration,
                    local.isrc.as_deref(),
                    &remote.title,
                    remote.duration,
                    remote.isrc.as_deref(),
                    false,
                )
            {
                hits.push((pos, remote));
            }
        }

        if hits.len() == 1 {
            let (pos, remote) = hits[0];
            claimed_positions.insert(pos);
            alignments.insert(
                local.path.clone(),
                TrackAlignment {
                    local_path: local.path.clone(),
                    remote_track_id: remote.id.clone(),
                    disc_number: pos.0,
                    track_number: pos.1,
                },
            );
        } else if !hits.is_empty() {
            conflicts.push(format!("Multiple candidates for track {}", local.title));
        }
    }

    let missing_remote_track_ids: Vec<String> = remote_release
        .tracks
        .iter()
        .filter(|t| !claimed_positions.contains(&(t.disc_number, t.track_number)))
        .map(|t| t.id.clone())
        .collect();

    let matched_count = alignments.len();
    let total_remote_tracks = remote_release.tracks.len();
    let compatible = conflicts.is_empty() && total_remote_tracks > 0;
    let incomplete = compatible && matched_count > 0 && !missing_remote_track_ids.is_empty();

    StructureMatchResult {
        compatible,
        incomplete,
        matched_count,
        total_remote_tracks,
        conflicts,
        alignments,
        missing_remote_track_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tidal::TidalTrack;

    #[test]
    fn cached_credit_aliases_reconcile_mix_names_without_relaxing_global_matching() {
        let remote = TidalTrack {
            title: "Heartbeat (Totally Enormous Extinct Dinosaurs Remix)".into(),
            credits: serde_json::json!([{"name":"Totally Enormous Extinct Dinosaurs","contributor_id":42,"role":"Remixer"}]),
            ..Default::default()
        };
        let release = TidalRelease {
            tracks: vec![
                remote.clone(),
                TidalTrack {
                    artists: vec![serde_json::json!({"id":42,"name":"TEED"})],
                    credits: serde_json::json!([{"name":"TEED","contributor_id":"42","role":"Producer"}]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut aliases = CreditAliases::new();
        aliases.begin_batch();
        for track in &release.tracks {
            aliases.extend_releases([&TidalRelease {
                tracks: vec![track.clone()],
                ..Default::default()
            }]);
        }
        aliases.finish_batch();
        assert!(aliases.titles_match("Heartbeat (TEED Remix)", &remote, false));
        assert!(!aliases.titles_match("Heartbeat (Other Person Remix)", &remote, true));
        assert!(!aliases.titles_match("Heartbeat (TEED Extended Remix)", &remote, true));
        assert!(!aliases.titles_match("Heartbeat (TEED Radio Edit)", &remote, true));
        assert!(!aliases.titles_match("Different Song (TEED Remix)", &remote, true));
        assert!(!aliases.titles_match("Heartbeat (Remix)", &remote, false));
        assert!(aliases.titles_match("Heartbeat (Remix)", &remote, true));
        assert!(!aliases.titles_match("Heartbeat", &remote, true));
        assert!(!recording_matches(
            "Heartbeat (TEED Remix)",
            180.0,
            Some("ABC"),
            &remote.title,
            180.0,
            Some("ABC"),
            false
        ));
        let homonym = TidalRelease {
            tracks: vec![TidalTrack {
                artists: vec![serde_json::json!({"id":99,"name":"TEED"})],
                ..Default::default()
            }],
            ..Default::default()
        };
        aliases.begin_batch();
        aliases.extend_releases([&homonym]);
        aliases.finish_batch();
        assert!(
            !aliases.titles_match("Heartbeat (TEED Remix)", &remote, false),
            "Finishing a later cache batch must retain ambiguity introduced by an unrelated person"
        );
    }

    #[test]
    fn feature_annotations_require_the_person_on_the_compared_track() {
        let remote = TidalTrack {
            title: "İ Feel Alive".into(),
            artists: vec![serde_json::json!({"id":42,"name":"A Guest","type":"FEATURED"})],
            credits: serde_json::json!([{"name":"Another Guest","role":"Vocals"}]),
            ..Default::default()
        };
        let aliases = CreditAliases::new();
        assert!(aliases.titles_match("İ Feel Alive (feat. A Guest)", &remote, false));
        assert!(aliases.titles_match("İ Feel Alive feat. A Guest & Another Guest", &remote, false));
        assert!(!aliases.titles_match("İ Feel Alive (feat. Unknown Guest)", &remote, false));
        assert!(!aliases.titles_match("İ Feel Alive (Live)", &remote, false));
        assert!(!aliases.titles_match(
            "İ Feel Alive (feat. A Guest & Unknown Guest)",
            &remote,
            false
        ));
    }

    #[test]
    fn writing_and_production_credits_do_not_prove_a_featured_performance() {
        let mut aliases = CreditAliases::new();
        let performer_reference = TidalRelease {
            tracks: vec![TidalTrack {
                artists: vec![serde_json::json!({"id":55,"name":"Guest"})],
                ..Default::default()
            }],
            ..Default::default()
        };
        aliases.extend_releases([&performer_reference]);
        for role in [
            "Composer",
            "Songwriter",
            "Author",
            "Writer",
            "Producer",
            "Engineer",
        ] {
            let remote = TidalTrack {
                title: "Song".into(),
                credits: serde_json::json!([{"name":"Guest","contributor_id":55,"role":role}]),
                ..Default::default()
            };
            assert!(
                !aliases.titles_match("Song (feat. Guest)", &remote, false),
                "{role} alone does not prove that this version includes Guest's performance"
            );
            let artist = TidalTrack {
                artists: vec![serde_json::json!({"id":55,"name":"Guest"})],
                ..remote
            };
            assert!(aliases.titles_match("Song (feat. Guest)", &artist, false));
        }
        for role in [
            "Vocals",
            "Lead Vocalist",
            "Featured Artist",
            "Performer",
            "Singer",
        ] {
            let remote = TidalTrack {
                title: "Song".into(),
                credits: serde_json::json!([{"name":"Guest","contributor_id":55,"role":role}]),
                ..Default::default()
            };
            assert!(
                aliases.titles_match("Song (feat. Guest)", &remote, false),
                "{role}"
            );
        }
    }

    #[test]
    fn explicit_track_credit_disambiguates_a_homonymous_cached_alias() {
        let remote = TidalTrack {
            title: "Reviver (TEED Remix)".into(),
            credits: serde_json::json!([{"name":"TEED","contributor_id":3943034,"role":"Remixer"}]),
            ..Default::default()
        };
        let release = TidalRelease {
            tracks: vec![
                remote.clone(),
                TidalTrack {
                    artists: vec![
                        serde_json::json!({"id":3943034,"name":"TEED"}),
                        serde_json::json!({"id":61553209,"name":"Teed"}),
                    ],
                    credits: serde_json::json!([{"name":"Totally Enormous Extinct Dinosaurs","contributor_id":3943034}]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut aliases = CreditAliases::new();
        aliases.extend_releases([&release]);
        assert!(aliases.titles_match(
            "Reviver (Totally Enormous Extinct Dinosaurs Remix)",
            &remote,
            false
        ));
        let uncredited = TidalTrack {
            credits: serde_json::json!([]),
            ..remote.clone()
        };
        assert!(!aliases.titles_match(
            "Reviver (Totally Enormous Extinct Dinosaurs Remix)",
            &uncredited,
            false
        ));
        let conflicting = TidalTrack {
            credits: serde_json::json!([{"name":"TEED","contributor_id":3943034},{"name":"TEED","artist_id":61553209}]),
            ..remote
        };
        assert!(!aliases.titles_match(
            "Reviver (Totally Enormous Extinct Dinosaurs Remix)",
            &conflicting,
            false
        ));
    }

    #[test]
    fn omitted_generic_mix_word_requires_the_same_explicit_people_and_remixer() {
        let remote = TidalTrack {
            title: "Song (Person One, Person Two Mix)".into(),
            artists: vec![serde_json::json!({"id":11,"name":"Person Two"})],
            credits: serde_json::json!([{"name":"Person One","contributor_id":12,"role":"Remixer"}]),
            ..Default::default()
        };
        let release = TidalRelease {
            tracks: vec![remote.clone()],
            ..Default::default()
        };
        let mut aliases = CreditAliases::new();
        aliases.extend_releases([&release]);
        assert!(aliases.titles_match("Song (Person One & Person Two)", &remote, true));
        assert!(aliases.titles_match("Song (Person Two & Person One)", &remote, true));
        assert!(!aliases.titles_match("Song (Person One & Person Two)", &remote, false));
        for title in [
            "Different Song (Person One & Person Two)",
            "Song (Other Person & Person Two)",
            "Song (Person One)",
            "Song (Person One & Person Two Live)",
            "Song (Person One & Person Two Edit)",
        ] {
            assert!(!aliases.titles_match(title, &remote, true), "{title}");
        }
        for title in [
            "Song (Person One, Person Two Extended Mix)",
            "Song (Person One, Person Two Remix)",
            "Song (Person One, Person Two Radio Mix)",
        ] {
            let variant = TidalTrack {
                title: title.into(),
                ..remote.clone()
            };
            assert!(
                !aliases.titles_match("Song (Person One & Person Two)", &variant, true),
                "{title}"
            );
        }
        let no_remixer = TidalTrack {
            credits: serde_json::json!([{"name":"Person One","contributor_id":12,"role":"Composer"}]),
            ..remote
        };
        assert!(!aliases.titles_match("Song (Person One & Person Two)", &no_remixer, true));
    }

    #[test]
    fn homonymous_people_and_credit_resource_ids_do_not_establish_aliases() {
        let remote = TidalTrack {
            title: "Song (Person Remix)".into(),
            credits: serde_json::json!([{"name":"Person","contributor_id":1}]),
            ..Default::default()
        };
        let release = TidalRelease {
            tracks: vec![
                remote.clone(),
                TidalTrack {
                    credits: serde_json::json!([
                        {"name":"Alias","contributor_id":1},
                        {"name":"Alias","contributor_id":2},
                        {"name":"Resource Alias","id":"credit-1"},
                        {"name":"Person","id":"credit-1"}
                    ]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut aliases = CreditAliases::new();
        aliases.extend_releases([&release]);
        assert!(!aliases.titles_match("Song (Alias Remix)", &remote, false));
        assert!(!aliases.titles_match("Song (Resource Alias Remix)", &remote, false));
        let unbridged = TidalRelease {
            tracks: vec![TidalTrack {
                title: "Song (First Artist Remix)".into(),
                artists: vec![serde_json::json!({"id":9,"name":"First Artist"})],
                credits: serde_json::json!([{"contributor_id":9,"name":"Other Person"}]),
                ..Default::default()
            }],
            ..Default::default()
        };
        aliases.extend_releases([&unbridged]);
        assert!(
            !aliases.titles_match("Song (Other Person Remix)", &unbridged.tracks[0], false),
            "Matching numeric IDs alone cannot bridge people with no agreeing name"
        );
    }

    #[test]
    fn test_recording_matches_duration_and_title() {
        assert!(recording_matches(
            "Bohemian Rhapsody",
            354.0,
            None,
            "Bohemian Rhapsody",
            355.0,
            None,
            false
        ));

        // Duration > 3s difference fails
        assert!(!recording_matches(
            "Bohemian Rhapsody",
            350.0,
            None,
            "Bohemian Rhapsody",
            356.0,
            None,
            false
        ));

        // Conflicting mix name fails
        assert!(!recording_matches(
            "Song (Club Mix)",
            200.0,
            None,
            "Song (Acoustic Mix)",
            200.0,
            None,
            false
        ));
    }

    #[test]
    fn test_structure_match_exact() {
        let local = vec![
            LocalTrackInfo {
                path: "/music/01.flac".to_string(),
                title: "Track One".to_string(),
                artist: "Band".to_string(),
                album: "Album".to_string(),
                duration: 180.0,
                track_number: 1,
                disc_number: 1,
                isrc: None,
            },
            LocalTrackInfo {
                path: "/music/02.flac".to_string(),
                title: "Track Two".to_string(),
                artist: "Band".to_string(),
                album: "Album".to_string(),
                duration: 200.0,
                track_number: 2,
                disc_number: 1,
                isrc: None,
            },
        ];

        let release = TidalRelease {
            id: "rel_1".to_string(),
            artist: "Band".to_string(),
            title: "Album".to_string(),
            date: "2020-01-01".to_string(),
            r#type: "album".to_string(),
            available: Some(true),
            track_count: 2,
            explicit: false,
            copyright: None,
            label: None,
            quality: "LOSSLESS".to_string(),
            tracks: vec![
                TidalTrack {
                    id: "t1".to_string(),
                    title: "Track One".to_string(),
                    isrc: None,
                    track_number: 1,
                    disc_number: 1,
                    duration: 180.0,
                    bpm: None,
                    key: None,
                    key_scale: None,
                    copyright: None,
                    ..Default::default()
                },
                TidalTrack {
                    id: "t2".to_string(),
                    title: "Track Two".to_string(),
                    isrc: None,
                    track_number: 2,
                    disc_number: 1,
                    duration: 200.0,
                    bpm: None,
                    key: None,
                    key_scale: None,
                    copyright: None,
                    ..Default::default()
                },
            ],
            tracks_loaded: true,
            ..Default::default()
        };

        let result = structure_match(&local, &release);
        assert!(result.compatible);
        assert!(!result.incomplete);
        assert_eq!(result.matched_count, 2);
        assert!(result.missing_remote_track_ids.is_empty());
        assert_eq!(
            result
                .alignments
                .get("/music/01.flac")
                .unwrap()
                .remote_track_id,
            "t1"
        );
    }
}
