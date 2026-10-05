//! Pure recommendation policy. Evidence extraction stays at the database boundary.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Evidence comes only from present files and their verified online links. A
/// candidate never adds evidence to this profile merely by being recommended.
#[derive(Default, Debug)]
pub struct ArtistReferenceProfile {
    labels: HashMap<String, HashSet<String>>,
    rights: HashMap<String, HashSet<String>>,
    recordings: HashSet<String>,
    contributors: HashMap<Contributor, HashSet<String>>,
    credits_by_id: HashMap<(&'static str, String), HashSet<String>>,
    credits_by_name: HashMap<(&'static str, String), HashSet<String>>,
    known_credit_names: HashSet<(&'static str, String)>,
    genres: HashSet<String>,
    album_artists: HashSet<String>,
    album_artist_ids: HashSet<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Contributor {
    role: &'static str,
    name: String,
    id: Option<String>,
}

/// Cross-artist prevalence prevents a large distributor or ubiquitous studio
/// contributor from being treated as evidence of artist identity.
#[derive(Default, Debug)]
pub struct ReferenceCorpus {
    artists: usize,
    labels: HashMap<String, usize>,
    rights: HashMap<String, usize>,
    people: HashMap<String, usize>,
}

#[derive(Default, Debug)]
pub struct RecommendationEvidence {
    pub label_match: bool,
    pub label_supported: bool,
    pub rights_match: bool,
    pub rights_continuity: bool,
    pub recordings: usize,
    pub contributor_tracks: usize,
    pub contributors: usize,
    pub local_contributor_recordings: usize,
    pub genre_matches: usize,
    pub common_label: bool,
    pub common_rights: bool,
    pub common_contributors: usize,
    pub technical_contributors: usize,
    pub credited_main_tracks: usize,
    pub foreign_main_tracks: usize,
    pub conflicting_track_artists: bool,
    pub checked_without_identity_support: bool,
    pub reference_creative_recordings: usize,
    pub checked_creative_tracks: usize,
}

fn value_strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(value) => vec![value],
        Value::Array(values) => values.iter().flat_map(value_strings).collect(),
        Value::Object(value) => value
            .get("name")
            .or_else(|| value.get("text"))
            .map(value_strings)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn metadata_strings<'a>(metadata: &'a Value, keys: &[&str]) -> Vec<&'a str> {
    // Both canonical metadata and file snapshots are accepted. Tag aliases are
    // case insensitive, but names themselves are never split at commas.
    let mut result = Vec::new();
    for object in [metadata.get("tags"), Some(metadata)] {
        for (key, value) in object.and_then(Value::as_object).into_iter().flatten() {
            if keys.iter().any(|alias| key.eq_ignore_ascii_case(alias)) {
                result.extend(value_strings(value));
            }
        }
    }
    result
}

/// Compare named rights holders independently of copyright years and licensing
/// boilerplate. The entity name is retained; different labels never coalesce.
pub fn rights_holder_key(value: &str) -> String {
    let lower = value.to_lowercase().replace(['℗', '©'], " ");
    let mut holder = lower.as_str();
    for marker in [
        "under exclusive licen",
        "licensed exclusively to",
        "exclusively licensed to",
        "all rights reserved",
    ] {
        if let Some(position) = holder.find(marker) {
            holder = &holder[..position];
        }
    }
    let mut in_prefix = true;
    let words: Vec<_> = holder
        .split_whitespace()
        .filter(|word| {
            let plain = word.trim_matches(|c: char| !c.is_alphanumeric());
            if matches!(plain, "copyright" | "phonogram") || matches!(*word, "(p)" | "(c)") {
                return false;
            }
            let years: Vec<_> = plain.split(['-', '–', '/', ',']).collect();
            if in_prefix
                && years.iter().all(|year| {
                    year.len() == 4
                        && year
                            .parse::<u16>()
                            .is_ok_and(|year| (1900..=2099).contains(&year))
                })
            {
                return false;
            }
            if !plain.is_empty() {
                in_prefix = false;
            }
            !matches!(plain, "inc" | "llc" | "ltd" | "limited")
        })
        .collect();
    crate::matching::name_key(&words.join(" "))
}

fn role_group(role: &str) -> Option<&'static str> {
    match crate::matching::name_key(role).as_str() {
        "composer" | "composition" | "songwriter" | "writer" | "musicandlyrics" | "composedby"
        | "writtenby" => Some("writing"),
        "lyricist" | "lyrics" | "lyricsby" => Some("lyrics"),
        "producer" | "coproducer" | "associateproducer" | "additionalproducer" | "producedby" => {
            Some("production")
        }
        "engineer" | "recordingengineer" | "assistantengineer" | "mixer" | "mixingengineer"
        | "masterer" | "masteringengineer" => Some("technical"),
        // Performer, publisher and executive producer appearances are not
        // compositional/recording identity evidence.
        _ => None,
    }
}

fn string_id(value: &Value) -> Option<String> {
    let id = match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => return None,
    };
    (!id.trim().is_empty() && id != "0").then_some(id)
}

fn structured_credits(credits: &Value) -> Vec<Contributor> {
    credits
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|credit| {
            let role = role_group(
                credit["role"]
                    .as_str()
                    .or_else(|| credit["roleId"].as_str())?,
            )?;
            let name = crate::matching::name_key(
                credit["name"]
                    .as_str()
                    .or_else(|| credit["person"].as_str())?,
            );
            if name.is_empty() {
                return None;
            }
            let id = ["contributor_id", "contributorId", "person_id", "personId"]
                .iter()
                .find_map(|field| string_id(&credit[field]));
            Some(Contributor { role, name, id })
        })
        .collect()
}

fn release_identity(metadata: &Value) -> String {
    let title = metadata_strings(metadata, &["album", "release", "title"])
        .first()
        .copied()
        .unwrap_or("");
    let date = metadata_strings(metadata, &["date", "release_date", "releaseDate", "year"])
        .first()
        .copied()
        .unwrap_or("");
    // Alternate provider IDs for the same named edition are not independent
    // releases supporting the label/copyright identity.
    format!(
        "{}:{}",
        crate::matching::name_key(title),
        date.chars().take(4).collect::<String>()
    )
}

fn clean_isrc(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_uppercase)
        .collect()
}

impl ArtistReferenceProfile {
    /// Once a verified link supplies a recording identity, merge previously
    /// unidentified local copies without counting them as independent anchors.
    pub fn canonicalize_recording(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        for recordings in self
            .contributors
            .values_mut()
            .chain(self.credits_by_id.values_mut())
            .chain(self.credits_by_name.values_mut())
        {
            if recordings.remove(old) {
                recordings.insert(new.to_owned());
            }
        }
    }

    fn add_credit(&mut self, credit: Contributor, recording_key: &str) {
        self.credits_by_name
            .entry((credit.role, credit.name.clone()))
            .or_default()
            .insert(recording_key.to_owned());
        if let Some(id) = &credit.id {
            self.known_credit_names
                .insert((credit.role, credit.name.clone()));
            self.credits_by_id
                .entry((credit.role, id.clone()))
                .or_default()
                .insert(recording_key.to_owned());
        }
        self.contributors
            .entry(credit)
            .or_default()
            .insert(recording_key.to_owned());
    }

    fn add_release(&mut self, release: &Value) {
        let edition = release_identity(release);
        for label in metadata_strings(release, &["label", "record_label", "recordlabel"]) {
            let key = crate::matching::name_key(label);
            if !key.is_empty() {
                self.labels.entry(key).or_default().insert(edition.clone());
            }
        }
        for rights in metadata_strings(
            release,
            &[
                "copyright",
                "phonographic_copyright",
                "phonographicCopyright",
            ],
        ) {
            let key = rights_holder_key(rights);
            if !key.is_empty() {
                self.rights.entry(key).or_default().insert(edition.clone());
            }
        }
        for genre in metadata_strings(release, &["genre", "genres"]) {
            self.genres.extend(
                genre
                    .split(';')
                    .map(crate::matching::name_key)
                    .filter(|value| !value.is_empty()),
            );
        }
    }

    pub fn add_local_recording(&mut self, recording_key: &str, metadata: &Value) {
        self.add_release(metadata);
        for artist in metadata_strings(metadata, &["albumartist", "album_artist"]) {
            let name = crate::matching::name_key(artist);
            if !name.is_empty() {
                self.album_artists.insert(name);
            }
        }
        for isrc in metadata_strings(metadata, &["isrc"]) {
            let key = clean_isrc(isrc);
            if !key.is_empty() {
                self.recordings.insert(key);
            }
        }
        for role in [
            "composer",
            "lyricist",
            "songwriter",
            "producer",
            "engineer",
            "mixer",
        ] {
            for names in metadata_strings(metadata, &[role]) {
                for name in names
                    .split(';')
                    .map(crate::matching::name_key)
                    .filter(|value| !value.is_empty())
                {
                    self.add_credit(
                        Contributor {
                            role: role_group(role).unwrap(),
                            name,
                            id: None,
                        },
                        recording_key,
                    );
                }
            }
        }
    }

    pub fn add_verified_recording(&mut self, recording_key: &str, track: &Value, release: &Value) {
        self.add_release(release);
        for id in release["artist_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(string_id)
        {
            self.album_artist_ids.insert(id);
        }
        for isrc in metadata_strings(track, &["isrc"]) {
            let key = clean_isrc(isrc);
            if !key.is_empty() {
                self.recordings.insert(key);
            }
        }
        // Track copyright is often richer than its containing release header.
        let edition = release_identity(release);
        for rights in metadata_strings(track, &["copyright"]) {
            let key = rights_holder_key(rights);
            if !key.is_empty() {
                self.rights.entry(key).or_default().insert(edition.clone());
            }
        }
        for genre in metadata_strings(track, &["genre", "genres"]) {
            self.genres.extend(
                genre
                    .split(';')
                    .map(crate::matching::name_key)
                    .filter(|value| !value.is_empty()),
            );
        }
        for credit in structured_credits(&track["credits"]) {
            self.add_credit(credit, recording_key);
        }
    }

    pub fn evidence(
        &self,
        release: &Value,
        tracks: Option<&Vec<Value>>,
        corpus: &ReferenceCorpus,
    ) -> RecommendationEvidence {
        let mut evidence = RecommendationEvidence::default();
        let mut specific_label_match = false;
        let mut specific_rights_match = false;
        evidence.reference_creative_recordings = self.contributors.iter()
            .filter(|(credit, _)| credit.role != "technical"
                && (!corpus.common_person(credit) || self.album_artists.contains(&credit.name)))
            .flat_map(|(_, recordings)| recordings.iter()).collect::<HashSet<_>>().len();
        let candidate_has_label = metadata_strings(release, &["label", "record_label", "recordlabel"])
            .into_iter().any(|value| meaningful_entity(&crate::matching::name_key(value)));
        for label in metadata_strings(release, &["label", "record_label", "recordlabel"]) {
            let key = crate::matching::name_key(label);
            if let Some(editions) = self.labels.get(&key) {
                evidence.label_match = true;
                let common = corpus.common_entity(&key, &corpus.labels);
                specific_label_match |= !common;
                evidence.common_label |= common;
                evidence.label_supported |= !common && editions.len() >= 2;
            }
        }
        let mut candidate_rights = metadata_strings(
            release,
            &[
                "copyright",
                "phonographic_copyright",
                "phonographicCopyright",
            ],
        );
        for track in tracks.into_iter().flatten() {
            candidate_rights.extend(metadata_strings(track, &["copyright"]));
        }
        let candidate_has_rights = candidate_rights.iter()
            .any(|value| meaningful_entity(&rights_holder_key(value)));
        for rights in candidate_rights {
            let key = rights_holder_key(rights);
            if let Some(editions) = self.rights.get(&key) {
                evidence.rights_continuity = true;
                let common = corpus.common_entity(&key, &corpus.rights);
                specific_rights_match |= !common;
                evidence.common_rights |= common;
                evidence.rights_match |= !common && editions.len() >= 2;
            }
        }
        let candidate_genres: HashSet<_> = metadata_strings(release, &["genre", "genres"])
            .into_iter()
            .flat_map(|genre| genre.split(';'))
            .map(crate::matching::name_key)
            .collect();
        evidence.genre_matches = self.genres.intersection(&candidate_genres).count();
        let mut candidate_recordings = HashSet::new();
        let mut matching_recordings = HashSet::new();
        let mut local_recordings = HashSet::new();
        let mut people = HashSet::new();
        let mut common_people = HashSet::new();
        let mut technical_people = HashSet::new();
        let mut known_artist_appears = false;
        for track in tracks.into_iter().flatten() {
            let isrc = track["isrc"].as_str().map(clean_isrc).unwrap_or_default();
            let recording = if !isrc.is_empty() {
                format!("isrc:{isrc}")
            } else {
                format!(
                    "title:{}:duration:{}",
                    crate::matching::name_key(track["title"].as_str().unwrap_or("")),
                    track["duration"]
                )
            };
            if !candidate_recordings.insert(recording.clone()) {
                continue;
            }
            let artists = track["artists"].as_array();
            let known_artist = |artist: &Value| {
                artist["name"].as_str().is_some_and(|name| {
                    self.album_artists
                        .contains(&crate::matching::name_key(name))
                }) || string_id(&artist["id"]).is_some_and(|id| self.album_artist_ids.contains(&id))
            };
            known_artist_appears |= artists.into_iter().flatten().any(known_artist);
            let main_artists: Vec<_> = artists
                .into_iter()
                .flatten()
                .filter(|artist| {
                    artist["type"]
                        .as_str()
                        .or_else(|| artist["role"].as_str())
                        .is_some_and(|role| role.eq_ignore_ascii_case("MAIN"))
                        && (artist["name"]
                            .as_str()
                            .is_some_and(|name| !name.trim().is_empty())
                            || string_id(&artist["id"]).is_some())
                })
                .collect();
            if !main_artists.is_empty() {
                evidence.credited_main_tracks += 1;
                if !main_artists.into_iter().any(known_artist) {
                    evidence.foreign_main_tracks += 1;
                }
            }
            if !isrc.is_empty() && self.recordings.contains(&isrc) {
                evidence.recordings += 1;
            }
            let credits = structured_credits(&track["credits"]);
            if credits.iter().any(|credit| credit.role != "technical") {
                evidence.checked_creative_tracks += 1;
            }
            for candidate in credits {
                let name_key = (candidate.role, candidate.name.clone());
                let supported = candidate
                    .id
                    .as_ref()
                    .and_then(|id| self.credits_by_id.get(&(candidate.role, id.clone())))
                    .or_else(|| {
                        // A different known person must not regain a match
                        // through the weaker local tag with the same name.
                        if candidate.id.is_some() && self.known_credit_names.contains(&name_key) {
                            None
                        } else {
                            self.credits_by_name.get(&name_key)
                        }
                    });
                let Some(supported) = supported.filter(|recordings| !recordings.is_empty()) else {
                    continue;
                };
                // Count a person once across roles; cached IDs resolve aliases.
                let person_id = candidate.id.as_ref().or_else(|| {
                    self.contributors
                        .keys()
                        .find(|credit| credit.name == candidate.name && credit.id.is_some())
                        .and_then(|credit| credit.id.as_ref())
                });
                let person = person_id
                    .map(|id| format!("id:{id}"))
                    .unwrap_or_else(|| candidate.name.clone());
                if candidate.role == "technical" {
                    technical_people.insert(person);
                    continue;
                }
                if corpus.common_person(&candidate) && !self.album_artists.contains(&candidate.name)
                {
                    common_people.insert(person);
                    continue;
                }
                matching_recordings.insert(recording.clone());
                local_recordings.extend(supported.iter().cloned());
                people.insert(person);
            }
        }
        evidence.contributor_tracks = matching_recordings.len();
        evidence.contributors = people.len();
        evidence.local_contributor_recordings = local_recordings.len();
        evidence.common_contributors = common_people.len();
        evidence.technical_contributors = technical_people.len();
        // Missing track artist metadata is neutral. Positive evidence from a
        // genuine recording, creative collaborator or featured appearance also
        // protects legitimate producer albums, remixes and collaborations.
        evidence.conflicting_track_artists = evidence.credited_main_tracks >= 3
            && evidence.foreign_main_tracks.saturating_mul(5)
                >= evidence.credited_main_tracks.saturating_mul(4)
            && !known_artist_appears
            && evidence.recordings == 0
            && evidence.contributors == 0;
        // A completed, populated check can contradict artist-page attribution.
        // Missing, failed or stale checks remain neutral. Creative roles must
        // match: the same artist ID as a composer is not a producer credit.
        let complete = release["tracks_loaded"] == true
            && release["recommendation_track_snapshot_conflict"] != true
            && tracks.is_some_and(|tracks| !tracks.is_empty()
                && release["track_count"].as_u64() == Some(tracks.len() as u64)
                && tracks.iter().all(|track| track["credits_complete"] == true));
        let reference_has_rights = self.labels.keys().chain(self.rights.keys())
            .any(|key| meaningful_entity(key));
        evidence.checked_without_identity_support = complete
            && evidence.reference_creative_recordings >= 2
            && reference_has_rights
            && evidence.checked_creative_tracks > 0
            && (candidate_has_label || candidate_has_rights)
            && !specific_label_match
            && !specific_rights_match
            && evidence.recordings == 0
            && evidence.contributors == 0;
        evidence
    }
}

fn meaningful_entity(key: &str) -> bool {
    !matches!(key, "" | "unknown" | "none" | "na" | "notavailable" | "unspecified")
}

#[allow(clippy::too_many_arguments)]
pub fn recommendation_score_with_evidence(
    primary: bool,
    conflict: bool,
    catalogue: bool,
    evidence: &RecommendationEvidence,
    compilation: bool,
    unofficial: bool,
    official: bool,
) -> (i32, String, Vec<String>) {
    let mut score = 30;
    let mut reasons = Vec::new();
    if primary {
        score += 30;
        reasons.push("Verified album artist credit matches a linked artist".into());
    }
    if conflict {
        score -= 25;
        reasons.push("Album artist credits differ from the linked artist".into());
    } else if !primary {
        reasons.push(
            "Primary release credits are not cached; catalogue placement alone is insufficient"
                .into(),
        );
    }
    if catalogue {
        score += 5;
        reasons.push("Listed in a linked artist catalogue".into());
    }
    if evidence.label_match {
        score += if evidence.common_label {
            3
        } else if evidence.label_supported {
            20
        } else {
            10
        };
        reasons.push(if evidence.common_label {
            "Shared broad label/distributor is context, not artist identity evidence".into()
        } else if evidence.label_supported {
            "Record label matches at least two downloaded reference releases".into()
        } else {
            "Record label matches one downloaded reference release".into()
        });
    }
    if evidence.rights_continuity {
        score += if evidence.rights_match { 20 } else { 3 };
        reasons.push(if evidence.rights_match {
            "Copyright holder matches at least two downloaded reference releases, independent of year".into()
        } else if evidence.common_rights {
            "Shared corporate copyright holder is context, not artist identity evidence".into()
        } else {
            "Copyright holder matches one downloaded reference release".into()
        });
    }
    if evidence.recordings > 0 {
        score += 10;
        reasons.push(format!(
            "{} recording identifiers occur in the downloaded discography",
            evidence.recordings
        ));
    }
    let network = evidence.local_contributor_recordings >= 2
        && evidence.contributor_tracks >= 2
        && evidence.contributors >= 2;
    if evidence.contributors > 0 {
        score += if network { 20 } else { 5 };
        reasons.push(format!("{} shared writing/production contributors in matching roles across {} candidate recordings and {} independent downloaded recordings", evidence.contributors, evidence.contributor_tracks, evidence.local_contributor_recordings));
    }
    if evidence.common_contributors > 0 {
        reasons.push(format!(
            "{} widely shared contributors retained as context only",
            evidence.common_contributors
        ));
    }
    if evidence.technical_contributors > 0 {
        reasons.push(format!(
            "{} shared engineering/mixing contributors retained as context only",
            evidence.technical_contributors
        ));
    }
    if evidence.genre_matches > 0 {
        // Genre similarity is useful context but cannot authenticate a release.
        score += 2;
        reasons.push(format!(
            "{} shared genres; genre similarity alone does not confirm identity",
            evidence.genre_matches
        ));
    }
    if evidence.foreign_main_tracks > 0 {
        reasons.push(format!("{} of {} tracks with explicit main-artist credits name other artists{}", evidence.foreign_main_tracks, evidence.credited_main_tracks,
            if evidence.conflicting_track_artists { "; no downloaded-recording, creative-credit or artist-appearance evidence corroborates this release" } else { "; retained alongside recording and creative-credit evidence" }));
    }
    if evidence.conflicting_track_artists {
        score -= 20;
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
    if evidence.checked_without_identity_support {
        score = score.min(49);
        reasons.push(format!("All track credits checked; no shared recording, writing/production contributor, specific label or copyright holder with {} independent downloaded reference recordings. Artist-page attribution alone does not establish a match", evidence.reference_creative_recordings));
    }
    let strong = primary
        && !conflict
        && !evidence.conflicting_track_artists
        && !evidence.checked_without_identity_support
        && !compilation
        && !unofficial
        && (evidence.label_supported
            || evidence.rights_match
            || network
            || evidence.recordings >= 2);
    if strong && (network || evidence.recordings >= 2) {
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
    } else if conflict || unofficial || compilation || evidence.conflicting_track_artists
        || evidence.checked_without_identity_support {
        "Suspect"
    } else {
        "Potential"
    };
    (score, badge.into(), reasons)
}

impl ReferenceCorpus {
    pub fn from_profiles(profiles: &HashMap<String, ArtistReferenceProfile>) -> Self {
        let mut corpus = Self {
            artists: profiles.len(),
            ..Default::default()
        };
        for profile in profiles.values() {
            for label in profile.labels.keys() {
                *corpus.labels.entry(label.clone()).or_default() += 1;
            }
            for rights in profile.rights.keys() {
                *corpus.rights.entry(rights.clone()).or_default() += 1;
            }
            let people: HashSet<_> = profile
                .contributors
                .keys()
                .flat_map(|credit| {
                    std::iter::once(credit.name.clone())
                        .chain(credit.id.as_ref().map(|id| format!("id:{id}")))
                })
                .collect();
            for person in people {
                *corpus.people.entry(person).or_default() += 1;
            }
        }
        corpus
    }

    fn common(&self, occurrences: usize) -> bool {
        occurrences >= 20 || (occurrences >= 5 && occurrences.saturating_mul(10) >= self.artists)
    }

    fn common_entity(&self, key: &str, counts: &HashMap<String, usize>) -> bool {
        [
            "universalmusicgroup",
            "universalmusic",
            "sonymusic",
            "warnermusic",
            "distrokid",
            "tunecore",
            "cdbaby",
        ]
        .iter()
        .any(|company| key.contains(company))
            || matches!(key, "independent" | "selfreleased" | "unknown" | "none")
            || self.common(counts.get(key).copied().unwrap_or(0))
    }

    fn common_person(&self, credit: &Contributor) -> bool {
        self.common(self.people.get(&credit.name).copied().unwrap_or(0))
            || credit.id.as_ref().is_some_and(|id| {
                self.common(self.people.get(&format!("id:{id}")).copied().unwrap_or(0))
            })
    }
}

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

/// Prefer a newer, larger release only when every recording is positively
/// identified. Unknown track lists, unique mixes and territory failures stay visible.
pub fn subsumed_releases(
    releases: &[crate::tidal::TidalRelease],
) -> std::collections::HashMap<String, String> {
    use crate::release_matching::{clean_isrc, recording_matches};
    let eligible = |r: &crate::tidal::TidalRelease| {
        r.tracks_loaded
            && !r.tracks.is_empty()
            && r.available == Some(true)
            && r.official != Some(false)
            && !r.r#type.eq_ignore_ascii_case("compilation")
            && !r.artist.trim().is_empty()
            && r.date.len() >= 10
            && r.date
                .get(..10)
                .is_some_and(|d| d <= chrono::Utc::now().format("%Y-%m-%d").to_string().as_str())
    };
    let mut index: std::collections::HashMap<(String, String), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, r) in releases.iter().enumerate().filter(|(_, r)| eligible(r)) {
        for t in &r.tracks {
            if let Some(isrc) = clean_isrc(t.isrc.as_deref()).filter(|s| !s.is_empty()) {
                index
                    .entry((crate::matching::name_key(&r.artist), isrc))
                    .or_default()
                    .push(i);
            }
        }
    }
    let mut result = std::collections::HashMap::new();
    for source in releases.iter().filter(|r| eligible(r)) {
        let Some(isrc) = clean_isrc(source.tracks[0].isrc.as_deref()).filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(candidates) = index.get(&(crate::matching::name_key(&source.artist), isrc)) else {
            continue;
        };
        let best = candidates
            .iter()
            .map(|i| &releases[*i])
            .filter(|target| {
                if target.id == source.id
                    || target.date < source.date
                    || target.tracks.len() <= source.tracks.len()
                    || target.explicit != source.explicit
                    || target.audio_modes != source.audio_modes
                {
                    return false;
                }
                let mut used = HashSet::new();
                source.tracks.iter().all(|s| {
                    target.tracks.iter().enumerate().any(|(i, t)| {
                        let same = s.isrc.as_ref().is_some_and(|id| !id.is_empty())
                            && clean_isrc(s.isrc.as_deref()) == clean_isrc(t.isrc.as_deref())
                            && recording_matches(
                                &s.title,
                                s.duration,
                                s.isrc.as_deref(),
                                &t.title,
                                t.duration,
                                t.isrc.as_deref(),
                                true,
                            );
                        same && used.insert(i)
                    })
                })
            })
            .max_by(|a, b| {
                a.tracks
                    .len()
                    .cmp(&b.tracks.len())
                    .then_with(|| a.date.cmp(&b.date))
                    .then_with(|| a.id.cmp(&b.id))
            });
        if let Some(target) = best {
            result.insert(source.id.clone(), target.id.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_network_needs_independent_downloaded_recordings_and_matched_roles() {
        let release = serde_json::json!({"title":"Downloaded","date":"2020-01-01"});
        let track = serde_json::json!({"credits":[
            {"name":"Writer One","role":"Composer","contributor_id":1},
            {"name":"Writer Two","role":"Producer","contributor_id":2}
        ]});
        let mut profile = ArtistReferenceProfile::default();
        profile.add_verified_recording("local-one", &track, &release);
        // An alternate edition of the same downloaded recording is not a second anchor.
        profile.add_verified_recording(
            "local-one",
            &track,
            &serde_json::json!({"title":"Downloaded","id":"alternate"}),
        );
        profile.add_local_recording(
            "duplicate-local-copy",
            &serde_json::json!({"tags":{"composer":"Writer One","producer":"Writer Two"}}),
        );
        profile.canonicalize_recording("duplicate-local-copy", "local-one");
        let candidates = vec![
            serde_json::json!({"isrc":"GB123","credits":track["credits"]}),
            serde_json::json!({"isrc":"GB456","credits":track["credits"]}),
            serde_json::json!({"isrc":"GB-123","credits":track["credits"]}),
        ];
        let evidence = profile.evidence(&release, Some(&candidates), &ReferenceCorpus::default());
        assert_eq!(evidence.local_contributor_recordings, 1);
        assert_eq!(evidence.contributor_tracks, 2);
        assert_eq!(
            recommendation_score_with_evidence(true, false, true, &evidence, false, false, false).1,
            "Potential"
        );
        profile.add_verified_recording("local-two", &track, &release);
        let evidence = profile.evidence(&release, Some(&candidates), &ReferenceCorpus::default());
        assert_eq!(
            recommendation_score_with_evidence(true, false, true, &evidence, false, false, false).1,
            "Recommended"
        );
        assert_eq!(
            recommendation_score_with_evidence(true, true, true, &evidence, false, false, false).1,
            "Suspect"
        );
        assert_eq!(
            recommendation_score_with_evidence(true, false, true, &evidence, true, false, false).1,
            "Suspect"
        );
        let wrong_roles = vec![serde_json::json!({"isrc":"GB789","credits":[
            {"name":"Writer One","role":"Producer","contributor_id":1},
            {"name":"Writer Two","role":"Composer","contributor_id":2}
        ]})];
        assert_eq!(
            profile
                .evidence(&release, Some(&wrong_roles), &ReferenceCorpus::default())
                .contributors,
            0
        );
    }

    #[test]
    fn unrelated_main_artist_credits_are_suspect_unless_genuine_music_evidence_exists() {
        let mut profile = ArtistReferenceProfile::default();
        for (key, album) in [("a", "One"), ("b", "Two")] {
            profile.add_local_recording(key, &serde_json::json!({"tags":{
                "albumartist":"Library Artist","album":album,"date":"2020","label":"Small Label", "composer":"Known Writer"
            }}));
        }
        let candidate = serde_json::json!({"title":"Wrong Page Album","label":"Small Label"});
        let mut tracks: Vec<_> = (0..3).map(|number| serde_json::json!({
            "isrc":format!("GB{number}"),"artists":[{"id":"other","name":"Other Artist","type":"MAIN"}]
        })).collect();
        let corpus = ReferenceCorpus::default();
        let evidence = profile.evidence(&candidate, Some(&tracks), &corpus);
        assert!(evidence.conflicting_track_artists);
        assert_eq!(
            (evidence.foreign_main_tracks, evidence.credited_main_tracks),
            (3, 3)
        );
        assert_eq!(
            recommendation_score_with_evidence(true, false, true, &evidence, false, false, false).1,
            "Suspect"
        );
        let unknown: Vec<_> = tracks
            .iter()
            .cloned()
            .map(|mut track| {
                track["artists"] = serde_json::json!([]);
                track
            })
            .collect();
        assert!(
            !profile
                .evidence(&candidate, Some(&unknown), &corpus)
                .conflicting_track_artists
        );
        tracks[0]["artists"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id":"local","name":"Library Artist","type":"FEATURED"}));
        assert!(
            !profile
                .evidence(&candidate, Some(&tracks), &corpus)
                .conflicting_track_artists
        );
        tracks[0]["artists"].as_array_mut().unwrap().pop();
        tracks[0]["credits"] = serde_json::json!([{"name":"Known Writer","role":"Composer"}]);
        assert!(
            !profile
                .evidence(&candidate, Some(&tracks), &corpus)
                .conflicting_track_artists
        );
        tracks[0]["credits"] = serde_json::json!([]);
        profile.add_verified_recording(
            "a",
            &serde_json::json!({"isrc":"GB0"}),
            &serde_json::json!({"title":"One","date":"2020"}),
        );
        assert!(
            !profile
                .evidence(&candidate, Some(&tracks), &corpus)
                .conflicting_track_artists
        );
    }

    #[test]
    fn credit_ids_disambiguate_names_and_noncreative_credits_do_not_confirm_identity() {
        let mut profile = ArtistReferenceProfile::default();
        profile.add_local_recording("a", &serde_json::json!({"tags":{"composer":"Common Name"}}));
        profile.add_verified_recording(
            "a",
            &serde_json::json!({"credits":[
                {"name":"Common Name","role":"Composer","contributor_id":"right"},
                {"name":"Performer","role":"Vocals","contributor_id":"performer"},
                {"name":"Publisher","role":"Publisher","contributor_id":"publisher"},
                {"name":"Engineer","role":"Recording Engineer","contributor_id":"engineer"}
            ]}),
            &serde_json::json!({"title":"Reference"}),
        );
        let wrong = vec![serde_json::json!({"credits":[
            {"name":"Common Name","role":"Composer","contributor_id":"wrong"},
            {"name":"Performer","role":"Vocals","contributor_id":"performer"},
            {"name":"Publisher","role":"Publisher","contributor_id":"publisher"},
            {"name":"Engineer","role":"Recording Engineer","contributor_id":"engineer"}
        ]})];
        let evidence = profile.evidence(&Value::Null, Some(&wrong), &ReferenceCorpus::default());
        assert_eq!(evidence.contributors, 0);
        assert_eq!(evidence.technical_contributors, 1);
        let alias = vec![
            serde_json::json!({"credits":[{"name":"Different Spelling","role":"Songwriter","contributor_id":"right"}]}),
        ];
        assert_eq!(
            profile
                .evidence(&Value::Null, Some(&alias), &ReferenceCorpus::default())
                .contributors,
            1
        );
    }

    #[test]
    fn rights_years_and_broad_entities_do_not_distort_reference_identity() {
        assert_eq!(
            rights_holder_key("℗ 2020 Small Label Ltd. under exclusive license to Sony Music"),
            rights_holder_key("© 2025 Small Label")
        );
        assert_ne!(
            rights_holder_key("2020 Different Label"),
            rights_holder_key("2021 Small Label")
        );
        assert_eq!(rights_holder_key("© 2020 The 1975"), "the1975");
        let mut profiles = HashMap::new();
        for artist in 0..5 {
            let mut profile = ArtistReferenceProfile::default();
            for recording in 0..2 {
                let release = serde_json::json!({"title":format!("Album {recording}"), "date":"2020-01-01", "label":"Shared Label", "copyright":"2020 Sony Music", "genres":["Electronic"]});
                profile.add_verified_recording(&format!("r{recording}"), &serde_json::json!({"credits":[{"name":"Everywhere Producer","role":"Producer"}]}), &release);
            }
            profiles.insert(artist.to_string(), profile);
        }
        let corpus = ReferenceCorpus::from_profiles(&profiles);
        let tracks =
            vec![serde_json::json!({"credits":[{"name":"Everywhere Producer","role":"Producer"}]})];
        let evidence = profiles["0"].evidence(&serde_json::json!({"label":"Shared Label", "copyright":"2025 Sony Music", "genres":["Electronic"]}), Some(&tracks), &corpus);
        assert!(evidence.label_match && evidence.common_label);
        assert!(!evidence.label_supported && !evidence.rights_match);
        assert_eq!(evidence.common_contributors, 1);
        assert_eq!(evidence.contributors, 0);
        assert_eq!(evidence.genre_matches, 1);
        assert_eq!(
            recommendation_score_with_evidence(true, false, true, &evidence, false, false, true).1,
            "Potential"
        );
        // An artist's own creative credit remains relevant even when that
        // artist has contributed to many other artists in the local library.
        profiles.get_mut("0").unwrap().add_local_recording(
            "r0",
            &serde_json::json!({"tags":{"albumartist":"Everywhere Producer"}}),
        );
        assert_eq!(
            profiles["0"]
                .evidence(&Value::Null, Some(&tracks), &corpus)
                .contributors,
            1
        );
    }

    #[test]
    fn complete_disjoint_singles_cannot_use_a_contaminated_artist_id_as_identity() {
        let mut profile = ArtistReferenceProfile::default();
        for (key, copyright) in [("one", "2025 This Never Happened"), ("two", "2026 Colorize (Enhanced)")] {
            profile.add_verified_recording(key, &serde_json::json!({"isrc":format!("GB{key}"), "credits":[
                {"name":"Dragan Roganovic","role":"Composer","contributor_id":"writer"},
                {"name":"Dirty South","role":"Producer","contributor_id":"3518839"}
            ]}), &serde_json::json!({"title":key,"copyright":copyright,"artist_ids":["3518839"]}));
        }
        for credits in [
            serde_json::json!([{"name":"Domonique Parker","role":"Composer","contributor_id":"other"}]),
            serde_json::json!([{"name":"Dirty South","role":"Composer","contributor_id":"3518839"},
                {"name":"Steve Joines","role":"Lyricist","contributor_id":"unrelated"}]),
        ] {
            // Both a single and a two-track drop on the very same artist page.
            for total in [1, 2] {
                let tracks: Vec<_> = (0..total).map(|n| serde_json::json!({
                    "isrc":format!("US{n}"),"artists":[{"name":"Dirty South","id":"3518839","type":"MAIN"}],
                    "credits":credits,"credits_complete":true
                })).collect();
                let release = serde_json::json!({"tracks_loaded":true,"track_count":total,"copyright":"2026 ZoeBoy Records"});
                let evidence = profile.evidence(&release, Some(&tracks), &ReferenceCorpus::default());
                assert!(!evidence.conflicting_track_artists);
                assert_eq!(evidence.contributors, 0);
                assert!(evidence.checked_without_identity_support);
                let (score, badge, _) = recommendation_score_with_evidence(true, false, true, &evidence, false, false, true);
                assert_eq!(badge, "Suspect");
                assert!(score < 50);
            }
        }
    }

    #[test]
    fn absence_of_evidence_requires_complete_populated_checks_and_a_reference_history() {
        let mut profile = ArtistReferenceProfile::default();
        for key in ["one", "two"] {
            profile.add_verified_recording(key, &serde_json::json!({"isrc":format!("GB{key}"),"credits":[
                {"name":"Known Writer","role":"Composer","contributor_id":"writer"}
            ]}), &serde_json::json!({"title":key,"copyright":"2025 This Never Happened"}));
        }
        let release = serde_json::json!({"tracks_loaded":true,"track_count":1,"copyright":"2026 New Label"});
        let track = serde_json::json!({"isrc":"US123","credits_complete":true,"credits":[
            {"name":"Other Writer","role":"Composer","contributor_id":"other"}
        ]});
        let classify = |profile: &ArtistReferenceProfile, release: &Value, track: &Value| {
            let evidence = profile.evidence(release, Some(&vec![track.clone()]), &ReferenceCorpus::default());
            recommendation_score_with_evidence(true, false, true, &evidence, false, false, false).1
        };
        assert_eq!(classify(&profile, &release, &track), "Suspect");
        for (field, value) in [("tracks_loaded", serde_json::json!(false)),
            ("track_count", serde_json::json!(2)),
            ("recommendation_track_snapshot_conflict", serde_json::json!(true)),
            ("copyright", serde_json::json!("unknown"))] {
            let mut incomplete = release.clone();
            incomplete[field] = value;
            assert_eq!(classify(&profile, &incomplete, &track), "Potential", "{field}");
        }
        for (field, value) in [("credits_complete", serde_json::json!(false)),
            ("credits", serde_json::json!([]))] {
            let mut incomplete = track.clone();
            incomplete[field] = value;
            incomplete["credits_checked_at"] = serde_json::json!(123456); // A failed check is not completion.
            assert_eq!(classify(&profile, &release, &incomplete), "Potential", "{field}");
        }
        let mut same_writer = track.clone();
        same_writer["credits"] = serde_json::json!([{"name":"Known Writer","role":"Composer","contributor_id":"writer"}]);
        assert_eq!(classify(&profile, &release, &same_writer), "Potential"); // New labels are allowed.
        let mut same_recording = track.clone();
        same_recording["isrc"] = serde_json::json!("GBone");
        assert_eq!(classify(&profile, &release, &same_recording), "Potential");
        let mut same_rights = release.clone();
        same_rights["copyright"] = serde_json::json!("2026 This Never Happened");
        assert_ne!(classify(&profile, &same_rights, &track), "Suspect");
        let mut weak = ArtistReferenceProfile::default();
        weak.add_verified_recording("only-one", &same_writer, &same_rights);
        assert_eq!(classify(&weak, &release, &track), "Potential");
    }

    #[test]
    fn newer_release_must_contain_every_exact_recording() {
        use crate::tidal::{TidalRelease, TidalTrack};
        let track = TidalTrack {
            title: "Song".into(),
            isrc: Some("GB123".into()),
            duration: 180.0,
            ..Default::default()
        };
        let older = TidalRelease {
            id: "old".into(),
            artist: "Artist".into(),
            date: "2020-01-01".into(),
            tracks_loaded: true,
            available: Some(true),
            tracks: vec![track.clone()],
            ..Default::default()
        };
        let mut newer = TidalRelease {
            id: "new".into(),
            date: "2021-01-01".into(),
            tracks: vec![
                track.clone(),
                TidalTrack {
                    title: "Bonus".into(),
                    isrc: Some("GB456".into()),
                    duration: 200.0,
                    ..Default::default()
                },
            ],
            ..older.clone()
        };
        assert_eq!(
            subsumed_releases(&[older.clone(), newer.clone()])
                .get("old")
                .map(String::as_str),
            Some("new")
        );
        newer.tracks[0].title = "Song (Extended Mix)".into();
        assert!(subsumed_releases(&[older.clone(), newer.clone()]).is_empty());
        newer.tracks[0] = track;
        newer.available = Some(false);
        assert!(subsumed_releases(&[older, newer]).is_empty());
    }
}
