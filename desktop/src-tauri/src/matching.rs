use std::collections::HashSet;

pub fn norm(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    let mut result = String::with_capacity(lower.len());
    let mut prev_space = false;
    for c in lower.chars() {
        if c.is_alphanumeric() {
            result.push(c);
            prev_space = false;
        } else if !prev_space && !result.is_empty() {
            result.push(' ');
            prev_space = true;
        }
    }
    result.trim().to_string()
}

pub fn name_key(value: &str) -> String {
    norm(value).chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn title_key(value: &str) -> String {
    norm(value)
}

#[derive(Debug, Clone)]
pub struct ArtistMatchScore {
    pub artist_id: String,
    pub artist_name: String,
    pub score: i32,
    pub matched_releases: usize,
    pub local_releases: usize,
    pub exact_name: bool,
    pub matched_titles: Vec<String>,
    pub evidence: String,
}

pub fn score_artist_candidate(
    local_name: &str,
    local_album_titles: &[String],
    candidate_id: &str,
    candidate_name: &str,
    candidate_release_titles: &[String],
) -> ArtistMatchScore {
    let generic: HashSet<&str> = [
        "", "unknown", "unknown album", "single", "singles", "album",
        "greatest hits", "best of", "the best of", "untitled",
    ]
    .into_iter()
    .collect();

    let local_set: HashSet<String> = local_album_titles
        .iter()
        .map(|t| title_key(t))
        .filter(|t| !generic.contains(t.as_str()))
        .collect();

    let cand_set: HashSet<String> = candidate_release_titles
        .iter()
        .map(|t| title_key(t))
        .collect();

    let mut matched: Vec<String> = local_set.intersection(&cand_set).cloned().collect();
    matched.sort();
    let count = matched.len();

    let exact_name = !name_key(local_name).is_empty() && name_key(local_name) == name_key(candidate_name);
    let fraction = if !local_set.is_empty() {
        count as f64 / local_set.len() as f64
    } else {
        0.0
    };

    let score = if count > 0 {
        let support = if count == 1 {
            10
        } else {
            (20 + 4 * (count as i32 - 2)).min(33)
        };
        let base = if exact_name { 40 } else { 15 };
        let calc = base + (25.0 * fraction).round() as i32 + support;
        calc.min(98)
    } else {
        0
    };

    let evidence = format!(
        "{}/{} local release titles match · {} · track details not checked",
        count,
        local_set.len(),
        if exact_name { "artist name matches" } else { "artist name differs" }
    );

    ArtistMatchScore {
        artist_id: candidate_id.to_string(),
        artist_name: candidate_name.to_string(),
        score,
        matched_releases: count,
        local_releases: local_set.len(),
        exact_name,
        matched_titles: matched,
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_and_title_keys() {
        assert_eq!(name_key("The Beatles"), "thebeatles");
        assert_eq!(name_key("AC/DC"), "acdc");
        assert_eq!(title_key("A Night at the Opera (Deluxe Edition)"), "a night at the opera deluxe edition");
    }

    #[test]
    fn test_score_artist_candidate() {
        let local_name = "Queen";
        let local_albums = vec![
            "A Night at the Opera".to_string(),
            "News of the World".to_string(),
            "The Game".to_string(),
        ];
        let candidate_id = "12345";
        let candidate_name = "Queen";
        let cand_albums = vec![
            "A Night at the Opera".to_string(),
            "News of the World".to_string(),
            "Innuendo".to_string(),
        ];

        let score = score_artist_candidate(
            local_name,
            &local_albums,
            candidate_id,
            candidate_name,
            &cand_albums,
        );

        assert_eq!(score.matched_releases, 2);
        assert!(score.exact_name);
        assert!(score.score >= 75);
        assert_eq!(score.matched_titles.len(), 2);
    }
}
