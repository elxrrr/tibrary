//! Shared table filtering, including facets over the complete scoped dataset.
//! Values match the frontend's `readable` formatter, including missing cells.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, HashMap, HashSet}, sync::{Arc, atomic::Ordering}};
use unicode_normalization::UnicodeNormalization;
use crate::{actions, db::TursoDb, duplicates, workflows, preview_inputs, compare_table_cell, Backend};

#[derive(Debug, Default)]
pub struct ColumnSelection {
    include: Option<HashSet<String>>,
    exclude: HashSet<String>,
}

pub type ColumnFilters = BTreeMap<String, ColumnSelection>;

fn string_values(value: &Value) -> Result<HashSet<String>, String> {
    value.as_array().ok_or("Column filter selections must be lists")?
        .iter().map(|item| item.as_str().map(str::to_owned)
            .ok_or_else(|| "Column filter values must be text".to_string())).collect()
}

pub fn parse(value: Option<&Value>) -> Result<ColumnFilters, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else { return Ok(BTreeMap::new()); };
    let columns = value.as_object().ok_or("Column filters must be an object")?;
    columns.iter().map(|(column, value)| {
        let selection = if value.is_array() {
            // Accept the first contract for callers upgrading independently.
            ColumnSelection { include: Some(string_values(value)?), exclude: HashSet::new() }
        } else {
            let entry = value.as_object().ok_or("Each column filter must contain include or exclude selections")?;
            ColumnSelection {
                include: entry.get("include").map(string_values).transpose()?,
                exclude: entry.get("exclude").map(string_values).transpose()?.unwrap_or_default(),
            }
        };
        Ok((column.clone(), selection))
    }).collect()
}

pub fn readable(value: &Value) -> String {
    match value {
        Value::Null => "—".into(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(readable).collect::<Vec<_>>().join(" · "),
        Value::Object(items) => items.iter().map(|(key, item)|
            format!("{}: {}", key.replace('_', " "), readable(item)))
            .collect::<Vec<_>>().join(" · "),
        Value::Number(number) => number.as_f64().filter(|number| number.fract() == 0.0)
            .map(|number| format!("{number:.0}")).unwrap_or_else(|| number.to_string()),
        Value::Bool(value) => value.to_string(),
    }
}

pub fn matches(row: &Value, filters: &ColumnFilters, except: Option<&str>) -> bool {
    filters.iter().all(|(column, selection)| {
        if except == Some(column.as_str()) { return true; }
        let value = readable(&row[column]);
        selection.include.as_ref().is_none_or(|included| included.contains(&value))
            && !selection.exclude.contains(&value)
    })
}

pub fn filter_page(mut page: Value, filters: &ColumnFilters, offset: usize, limit: usize) -> Value {
    let rows = page["rows"].as_array_mut().map(std::mem::take).unwrap_or_default();
    let rows: Vec<_> = rows.into_iter().filter(|row| matches(row, filters, None)).collect();
    page["total"] = json!(rows.len());
    page["offset"] = json!(offset);
    page["rows"] = json!(rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>());
    page
}

pub fn facets(page: &Value, filters: &ColumnFilters, column: &str, search: &str, offset: usize, limit: usize) -> Value {
    let search = search.to_lowercase();
    let mut counts = BTreeMap::<String, usize>::new();
    for row in page["rows"].as_array().into_iter().flatten().filter(|row| matches(row, filters, Some(column))) {
        let value = readable(&row[column]);
        if value.to_lowercase().contains(&search) { *counts.entry(value).or_default() += 1; }
    }
    let mut options: Vec<_> = counts.into_iter().collect();
    options.sort_by(|(left, _), (right, _)| {
        match (left.parse::<f64>(), right.parse::<f64>()) {
            (Ok(left), Ok(right)) => left.total_cmp(&right),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            _ => left.to_lowercase().cmp(&right.to_lowercase()).then_with(|| left.cmp(right)),
        }
    });
    let total = options.len();
    json!({"options":options.into_iter().skip(offset).take(limit).map(|(value,count)|
        json!({"label":if value.is_empty(){"Blank"}else{value.as_str()},"value":value,"count":count}))
        .collect::<Vec<_>>(),"total":total,"offset":offset,"revision":page["revision"]})
}

fn matches_link_child(row: &Value, filters: &ColumnFilters, except: Option<&str>) -> bool {
    filters.iter().all(|(column, selection)| {
        if column == "tracks" || except == Some(column.as_str()) { return true; }
        let value = readable(&row[column]);
        if column == "artist" {
            let key = artist_key(&value);
            return selection.include.as_ref().is_none_or(|included| included.iter().any(|value| artist_key(value) == key))
                && !selection.exclude.iter().any(|value| artist_key(value) == key);
        }
        selection.include.as_ref().is_none_or(|included| included.contains(&value))
            && !selection.exclude.contains(&value)
    })
}

fn matches_link_count(row: &Value, filters: &ColumnFilters) -> bool {
    filters.get("tracks").is_none_or(|selection| {
        let value = readable(&row["tracks"]);
        selection.include.as_ref().is_none_or(|included| included.contains(&value))
            && !selection.exclude.contains(&value)
    })
}

fn link_position(row: &Value) -> (u32, u32) {
    // LinkRow positions include padded numbers and release totals. Compare the
    // indices, not their string width (Disc 2 must precede Disc 10).
    let position = row["position"].as_str().unwrap_or("");
    let index = |label: &str| position.split(label).nth(1)
        .and_then(|part| part.trim().split('/').next())
        .and_then(|number| number.trim().parse::<u32>().ok()).unwrap_or(0);
    (index("Disc "), index("Track "))
}

fn artist_key(artist: &str) -> String {
    // Compilation aliases are one library grouping, never one remote artist.
    // Other names retain punctuation, so similarly named artists stay separate.
    match crate::matching::name_key(artist).as_str() {
        "variousartists" | "variousartist" | "va" => "various artists".into(),
        _ => artist.trim().nfc().collect::<String>().to_lowercase(),
    }
}

#[derive(Default)]
struct LinkCounts { linked: usize, choices: usize, unlinked: usize, ignored: usize }

impl LinkCounts {
    fn from_tracks(tracks: &[Value]) -> Self {
        let mut counts = Self::default();
        for track in tracks {
            match track["status"].as_str().unwrap_or("") {
                "Ignored" => counts.ignored += 1,
                "Linked" => counts.linked += 1,
                "Needs choice" => counts.choices += 1,
                _ => counts.unlinked += 1,
            }
        }
        counts
    }
    fn status(&self, total: usize) -> &'static str {
        if self.ignored == total { "Ignored" }
        else if self.choices > 0 { "Needs choice" }
        else if self.unlinked > 0 { "Unlinked" }
        else { "Linked" }
    }
    fn evidence(&self, mut summary: Vec<String>) -> String {
        for (number, label) in [(self.linked, "linked"),
            (self.choices, if self.choices == 1 { "needs choice" } else { "need choice" }),
            (self.unlinked, "unlinked"), (self.ignored, "ignored")]
        {
            if number > 0 { summary.push(format!("{number} {label}")); }
        }
        summary.join(" · ")
    }
    fn apply(&self, row: &mut Value) {
        row["linked_tracks"] = json!(self.linked);
        row["unlinked_tracks"] = json!(self.unlinked);
        row["choice_tracks"] = json!(self.choices);
        row["ignored_tracks"] = json!(self.ignored);
    }
}

/// Link releases is a presentation of local release folders. Existing flat
/// LinkRows remain the source of truth for track filters, links and file actions.
fn link_release_groups(page: &Value, filters: &ColumnFilters, except: Option<&str>) -> Vec<Value> {
    let mut groups = BTreeMap::<(String, String, String), Vec<Value>>::new();
    for row in page["rows"].as_array().into_iter().flatten()
        .filter(|row| matches_link_child(row, filters, except))
    {
        let folder = duplicates::extract_release_folder(row["path"].as_str().unwrap_or(""))
            .nfc().collect::<String>();
        let canonical = |key: &str| row[key].as_str().unwrap_or("")
            .trim().nfc().collect::<String>().to_lowercase();
        groups.entry((folder, artist_key(row["artist"].as_str().unwrap_or("")), canonical("release")))
            .or_default().push(row.clone());
    }
    groups.into_iter().map(|(key, mut children)| {
        children.sort_by(|left, right| link_position(left).cmp(&link_position(right))
            .then_with(|| compare_table_cell(left, right, "id")));
        let count = children.len();
        let counts = LinkCounts::from_tracks(&children);
        let evidence = counts.evidence(vec![format!("{count} {}", if count == 1 { "track" } else { "tracks" })]);
        let identity = serde_json::to_vec(&key).expect("release key serializes");
        let id = format!("local-release:{:x}", Sha256::digest(identity));
        let track_ids: Vec<_> = children.iter().map(|child| child["id"].clone()).collect();
        let mut row = json!({"id":id,"artist":children[0]["artist"],"release":children[0]["release"],
            "tracks":count,"status":counts.status(count),"evidence":evidence,"path":key.0,
            "track_ids":track_ids,"children":children,"expanded_available":true,"link_group":true,
            "ignored":counts.ignored == count});
        counts.apply(&mut row);
        row
    }).collect()
}

fn link_artist_groups(releases: Vec<Value>, root: &str) -> Vec<Value> {
    let mut artists = BTreeMap::<String, Vec<Value>>::new();
    for release in releases {
        artists.entry(artist_key(release["artist"].as_str().unwrap_or("")))
            .or_default().push(release);
    }
    artists.into_iter().map(|(key, mut children)| {
        children.sort_by(|left, right| compare_table_cell(left, right, "release")
            .then_with(|| compare_table_cell(left, right, "path")));
        let tracks: Vec<Value> = children.iter().flat_map(|release|
            release["children"].as_array().into_iter().flatten().cloned()).collect();
        let counts = LinkCounts::from_tracks(&tracks);
        let release_count = children.len();
        let release_label = format!("{release_count} {}", if release_count == 1 { "release" } else { "releases" });
        let track_count = tracks.len();
        let evidence = counts.evidence(vec![release_label.clone(),
            format!("{track_count} {}", if track_count == 1 { "track" } else { "tracks" })]);
        let id = format!("local-artist:{:x}", Sha256::digest(serde_json::to_vec(
            &(root.trim_end_matches('/').nfc().collect::<String>(), &key)).expect("artist key serializes")));
        let artist = if key == "various artists" { json!("Various Artists") } else { children[0]["artist"].clone() };
        let mut row = json!({"id":id,"artist":artist,"artist_group":true,"release":release_label,
            "releases":release_count,"tracks":track_count,"status":counts.status(track_count),
            "evidence":evidence,"children":children,"expanded_available":true,
            "track_ids":tracks.iter().map(|track|track["id"].clone()).collect::<Vec<_>>(),
            "ignored":counts.ignored == track_count});
        counts.apply(&mut row);
        row
    }).collect()
}

fn link_presentation_groups(page: &Value, filters: &ColumnFilters, args: &Value, except: Option<&str>) -> Vec<Value> {
    let releases = link_release_groups(page, filters, except);
    if args["group_artists"] == true {
        link_artist_groups(releases, page["root"].as_str().or_else(|| args["root"].as_str()).unwrap_or(""))
    } else { releases }
}

fn grouped_link_page(mut page: Value, filters: &ColumnFilters, args: &Value, offset: usize, limit: usize) -> Value {
    let mut rows = link_presentation_groups(&page, filters, args, None);
    rows.retain(|row| matches_link_count(row, filters));
    let key = args["sort"].as_str().unwrap_or("artist");
    let key = if args["group_artists"] == true && key == "release" { "releases" } else { key };
    let descending = args["direction"] == "desc";
    rows.sort_by(|left, right| {
        let order = compare_table_cell(left, right, key);
        (if descending { order.reverse() } else { order })
            .then_with(|| compare_table_cell(left, right, "artist"))
            .then_with(|| compare_table_cell(left, right, "release"))
            .then_with(|| compare_table_cell(left, right, "path"))
            .then_with(|| compare_table_cell(left, right, "id"))
    });
    page["total"] = json!(rows.len());
    if args["group_artists"] == true {
        page["artist_total"] = json!(rows.len());
        page["release_total"] = json!(rows.iter().map(|row| row["releases"].as_u64().unwrap_or(0)).sum::<u64>());
    }
    page["track_total"] = json!(rows.iter().map(|row| row["tracks"].as_u64().unwrap_or(0)).sum::<u64>());
    page["offset"] = json!(offset);
    page["rows"] = json!(rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>());
    page
}

fn grouped_link_facets(page: &Value, filters: &ColumnFilters, args: &Value, column: &str, search: &str, offset: usize, limit: usize) -> Value {
    let mut groups = link_presentation_groups(page, filters, args, Some(column));
    if column != "tracks" { groups.retain(|row| matches_link_count(row, filters)); }
    // Counts are a release-level column. All other values and counts continue
    // to describe the matching tracks, including different statuses in one release.
    let mut rows = if column == "tracks" { groups } else {
        groups.into_iter().flat_map(|mut group| {
            let children = group["children"].as_array_mut().map(std::mem::take).unwrap_or_default();
            if group["artist_group"] == true {
                children.into_iter().flat_map(|mut release| release["children"].as_array_mut()
                    .map(std::mem::take).unwrap_or_default()).collect()
            } else { children }
        }).collect()
    };
    if column == "artist" {
        // Facets use the same compilation label as artist grouping without
        // changing original album-artist tags on the table's leaf tracks.
        for row in &mut rows {
            if artist_key(row["artist"].as_str().unwrap_or("")) == "various artists" {
                row["artist"] = json!("Various Artists");
            }
        }
    }
    facets(&json!({"rows":rows,"revision":page["revision"]}), &ColumnFilters::new(), column, search, offset, limit)
}

fn decorate_local_releases(mut page: Value, releases: Vec<Value>, mappings: &[Value], dates: &HashMap<String, String>, favourites: bool) -> Value {
    let mut by_artist = HashMap::<String, Vec<Value>>::new();
    for mut release in releases {
        release["date"] = json!(release["path"].as_str().and_then(|path| dates.get(path)).cloned().unwrap_or_default());
        release["local_release"] = json!(true);
        release["row_kind"] = json!("artist_release");
        release["link_group"] = json!(false);
        release["expanded_available"] = json!(false);
        // This view expands to releases only. Track IDs remain internal to the
        // Link releases view, so artist actions cannot receive file identifiers.
        if let Some(object) = release.as_object_mut() {
            object.remove("children");
            object.remove("track_ids");
        }
        by_artist.entry(artist_key(release["artist"].as_str().unwrap_or(""))).or_default().push(release);
    }
    for releases in by_artist.values_mut() {
        releases.sort_by(|left, right| compare_table_cell(left, right, "release")
            .then_with(|| compare_table_cell(left, right, "path")));
    }
    let mapped: HashMap<_, _> = mappings.iter().map(|mapping|
        (artist_key(mapping["artist"].as_str().unwrap_or("")), mapping)).collect();
    let resolved_id = |mapping: &Value| (mapping["resolved"] == true)
        .then(|| mapping["online_id"].as_str()).flatten().filter(|id| !id.is_empty()).map(str::to_owned);
    let mut aliases_by_id = HashMap::<String, Vec<String>>::new();
    for (key, mapping) in &mapped {
        if by_artist.contains_key(key) {
            if let Some(id) = resolved_id(mapping) { aliases_by_id.entry(id).or_default().push(key.clone()); }
        }
    }
    let favourite_ids: HashSet<String> = page["rows"].as_array().into_iter().flatten()
        .filter(|row| row["id"].as_str().is_some_and(|id|id.starts_with("favourite:")))
        .filter_map(|row| row["online_id"].as_str().filter(|id|!id.is_empty()).map(str::to_owned)).collect();
    if let Some(rows) = page["rows"].as_array_mut() {
        if favourites {
            rows.retain(|row| !row["id"].as_str().is_some_and(|id|id.starts_with("local:")) ||
                !mapped.get(&artist_key(row["artist"].as_str().unwrap_or("")))
                    .and_then(|mapping|resolved_id(mapping)).is_some_and(|id|favourite_ids.contains(&id)));
        }
        for row in rows {
            let key = artist_key(row["artist"].as_str().unwrap_or(""));
            let mapping = mapped.get(&key).copied();
            let online_favourite = favourites && row["id"].as_str().is_some_and(|id|id.starts_with("favourite:"));
            let online_id = row["online_id"].as_str().unwrap_or("").to_string();
            let mut local_keys = if online_favourite { aliases_by_id.get(&online_id).cloned().unwrap_or_default() } else { vec![] };
            // A matching name is a useful local association only when a saved
            // confirmed ID does not identify a different remote artist.
            let same_name_compatible = !online_favourite || mapping.and_then(resolved_id).is_none_or(|id|id == online_id);
            if by_artist.contains_key(&key) && same_name_compatible { local_keys.push(key.clone()); }
            local_keys.sort();
            local_keys.dedup();
            if favourites {
                let linked = if online_favourite { local_keys.iter().any(|key|
                    mapped.get(key).and_then(|mapping|resolved_id(mapping)).is_some_and(|id|id == online_id)) }
                    else { mapping.and_then(resolved_id).is_some() };
                row["catalogue_status"] = json!(if linked { "Linked" } else { "Not linked" });
                if row["online_id"].as_str().is_none_or(str::is_empty) && linked {
                    row["online_id"] = mapping.unwrap()["online_id"].clone();
                }
            }
            let mut children: Vec<Value> = local_keys.iter().flat_map(|key|
                by_artist.get(key).into_iter().flatten().cloned()).collect();
            children.sort_by(|left, right| compare_table_cell(left, right, "release")
                .then_with(|| compare_table_cell(left, right, "path")));
            if favourites {
                let aliases: Vec<Value> = local_keys.iter().filter_map(|key|
                    mapped.get(key).map(|mapping|mapping["artist"].clone())
                        .or_else(||by_artist.get(key).and_then(|releases|releases.first()).map(|release|release["artist"].clone()))).collect();
                let primary_key = local_keys.iter().max_by(|left, right| {
                    let count = |key: &String| by_artist.get(key).into_iter().flatten()
                        .map(|release| release["tracks"].as_u64().unwrap_or(0)).sum::<u64>();
                    count(left).cmp(&count(right)).then_with(||right.cmp(left))
                });
                row["local_aliases"] = json!(aliases);
                if let Some(primary) = primary_key {
                    row["lookup_artist"] = mapped.get(primary).map(|mapping|mapping["artist"].clone())
                        .or_else(||by_artist.get(primary).and_then(|releases|releases.first()).map(|release|release["artist"].clone()))
                        .unwrap_or_else(||row["artist"].clone());
                }
                row["tracks"] = json!(children.iter().map(|release|release["tracks"].as_u64().unwrap_or(0)).sum::<u64>());
                if online_favourite { row["status"] = json!(if children.is_empty() { "Missing locally" } else { "In library" }); }
            }
            let mut linked = 0u64;
            let mut unlinked = 0u64;
            let mut choices = 0u64;
            let mut ignored = 0u64;
            for child in &mut children {
                child["parent_id"] = row["id"].clone();
                linked += child["linked_tracks"].as_u64().unwrap_or(0);
                unlinked += child["unlinked_tracks"].as_u64().unwrap_or(0);
                choices += child["choice_tracks"].as_u64().unwrap_or(0);
                ignored += child["ignored_tracks"].as_u64().unwrap_or(0);
            }
            row["artist_group"] = json!(true);
            row["expanded_available"] = json!(!children.is_empty());
            row["release"] = json!(children.len());
            row["releases"] = json!(children.len());
            row["children"] = json!(children);
            row["linked_tracks"] = json!(linked);
            row["unlinked_tracks"] = json!(unlinked);
            row["choice_tracks"] = json!(choices);
            row["ignored_tracks"] = json!(ignored);
            let release_count = row["releases"].as_u64().unwrap_or(0);
            let summary = LinkCounts { linked: linked as usize, choices: choices as usize,
                unlinked: unlinked as usize, ignored: ignored as usize }.evidence(vec![
                    format!("{release_count} local {}", if release_count == 1 { "release" } else { "releases" })]);
            let evidence = row["evidence"].as_str().filter(|evidence| !evidence.is_empty());
            row["evidence"] = json!(evidence.map(|evidence|format!("{evidence} · {summary}")).unwrap_or(summary));
        }
    }
    if let Some(rows) = page["rows"].as_array() { page["total"] = json!(rows.len()); }
    page
}

fn filter_favourite_presentation(mut page: Value, args: &Value) -> Value {
    if let Some(rows) = page["rows"].as_array_mut() {
        if let Some(filter) = args["filter"].as_str().filter(|filter| !filter.is_empty() && *filter != "all") {
            rows.retain(|row| row["status"].as_str() == Some(filter));
        }
        if let Some(search) = args["search"].as_str().map(|search|search.trim().to_lowercase()).filter(|search|!search.is_empty()) {
            rows.retain(|row| row["artist"].as_str().unwrap_or("").to_lowercase().contains(&search) ||
                row["local_aliases"].as_array().into_iter().flatten().any(|alias|
                    alias.as_str().unwrap_or("").to_lowercase().contains(&search)));
        }
        page["total"] = json!(rows.len());
    }
    page
}

async fn artist_release_page(db: &TursoDb, args: &Value, page: Value) -> Result<Value, String> {
    let root = args["root"].as_str().filter(|root|!root.is_empty());
    let market = args["market"].as_str().unwrap_or("GB");
    let indexed = db.get_local_files_for_release_tables(root).await?;
    let mut roots = HashSet::new();
    let mut dates = HashMap::<String, String>::new();
    for file in indexed {
        roots.insert(file.root.trim_end_matches('/').to_string());
        let tags = workflows::extract_tags_map(&file.metadata);
        if let Some(date) = tags.get("date").filter(|date| !date.is_empty()) {
            let folder = duplicates::extract_release_folder(&file.path).nfc().collect::<String>();
            let entry = dates.entry(folder).or_insert_with(||date.clone());
            if date < entry { *entry = date.clone(); }
        }
    }
    let mut tracks = Vec::new();
    for root in roots {
        let page = db.get_link_rows(market, &root, None, None, None, None, 0, usize::MAX).await?;
        tracks.extend(page.rows.into_iter().map(|mut track| {
            if track.ignored { track.status = "Ignored".into(); }
            serde_json::to_value(track).expect("LinkRow serializes")
        }));
    }
    let releases = link_release_groups(&json!({"rows":tracks}), &ColumnFilters::new(), None);
    let favourites = args["route"] == "favourites";
    let mappings = if favourites {
        db.get_artist_rows(root, None, None, None, None, 0, usize::MAX).await?.rows
    } else { page["rows"].as_array().cloned().unwrap_or_default() };
    Ok(decorate_local_releases(page, releases, &mappings, &dates, favourites))
}

/// The table source always retains its existing scope/search rules. Column
/// selection and facets share this path and are applied before pagination.
async fn get_table_page(state: &Arc<Backend>, db: &TursoDb, args: &Value) -> Result<Value, String> {
    if matches!(args["route"].as_str(), Some("links" | "files"))
    {
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let root_opt = args.get("root").and_then(|v| v.as_str());
        let root = if let Some(r) = root_opt {
            r.to_string()
        } else {
            let roots = db.list_roots(market).await?;
            roots.into_iter().next().map(|r| r.root).unwrap_or_default()
        };
        if root.is_empty() {
            return Ok(json!({
                "rows": [],
                "total": 0,
                "offset": 0,
                "revision": 0,
                "preview_id": null
            }));
        }
        let filter = args.get("filter").and_then(|v| v.as_str());
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

        let page = db
            .get_link_rows(
                market, &root, filter, search, sort, direction, offset, limit,
            )
            .await?;
        let mut result = serde_json::to_value(page).map_err(|e| e.to_string())?;
        result["root"] = json!(root);
        return Ok(result);
    }
    if args["route"] == "missing"
    {
        let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
        let timeline = args.get("timeline").and_then(|v| v.as_str());
        let recommendation = args.get("recommendation").and_then(|v| v.as_str());
        let status_filter = args
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("filter").and_then(|v| v.as_str()));
        let type_filter = args.get("type").and_then(|v| v.as_str());
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

        let page = db
            .get_missing_rows_scoped_including_unavailable(
                market,
                timeline,
                recommendation,
                args.get("artist_scope").and_then(Value::as_str),
                status_filter,
                type_filter,
                search,
                sort,
                direction,
                offset,
                limit,
                args["include_unavailable"] == true,
            )
            .await?;
        let missing = db.get_missing_rows(market, Some("All missing releases"), None, None, None, None, None, None, 0, 0).await?;
        let mut value = serde_json::to_value(page).map_err(|e|e.to_string())?;
        value["missing_total"] = json!(missing.total);
        return Ok(value);
    }
    {
        let route = args
            .get("route")
            .and_then(|v| v.as_str())
            .unwrap_or("files");
        let search = args.get("search").and_then(|v| v.as_str());
        let sort = args.get("sort").and_then(|v| v.as_str());
        let direction = args.get("direction").and_then(|v| v.as_str());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        let filter = args.get("filter").and_then(|v| v.as_str());

        let operation = if route == "correct" || route == "organise" {
            args["action"].as_str().unwrap_or("dates")
        } else {
            route
        };
        let mut preview = {
            let previews = state.previews.lock().unwrap();
            args["preview_id"]
                .as_str()
                .and_then(|id| previews.get(id))
                .or_else(|| {
                    previews
                        .values()
                        .filter(|p| {
                            p["root"] == args["root"] && p["operation"].as_str() == Some(operation)
                        })
                        .max_by_key(|p| p["created"].as_i64().unwrap_or(0))
                })
                .cloned()
        };
        if matches!(route,"correct"|"organise") {
            let source=preview_inputs(db,operation).await?;
            preview=preview.filter(|p|p["source_inputs"]==source && p["root"]==args["root"] && p["operation"]==operation);
        }
        let mut cached_rows = if let Some(ref p) = preview {
            p["rows"].as_array().cloned()
        } else if ["mqa", "local", "online"].contains(&route) {
            Some(
                db.get_preference(&format!(
                    "desktop-{route}:{}",
                    args["root"].as_str().unwrap_or("")
                ))
                .await?
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default(),
            )
        } else {
            None
        };
        let local_manifest_saved = if route == "local" {
            db.get_preference(&format!(
                "desktop-local-manifest:{}",
                args["root"].as_str().unwrap_or("")
            ))
            .await?
        } else {
            None
        };
        if route == "local"
            && (local_manifest_saved.is_some()
                || cached_rows.as_ref().is_some_and(|rows| !rows.is_empty()))
        {
            let root = args["root"].as_str().unwrap_or("");
            let current = duplicates::indexed_manifest_fingerprint(db, root).await?;
            let saved = local_manifest_saved
                .as_ref()
                .and_then(|value| value.as_str());
            if saved != Some(current.as_str()) {
                let indexed = actions::files(db, root).await?;
                let rows = duplicates::clusters_to_group_rows(
                    &duplicates::find_duplicate_clusters(&indexed),
                );
                db.set_preference(&format!("desktop-local:{root}"), &json!(rows))
                    .await?;
                db.set_preference(&format!("desktop-local-manifest:{root}"), &json!(current))
                    .await?;
                cached_rows = Some(rows);
            }
        }
        if route == "mqa" {
            let root = args["root"].as_str().unwrap_or("");
            let current = duplicates::indexed_manifest_fingerprint(db, root).await?;
            let saved = db.get_preference(&format!("desktop-mqa-manifest:{root}")).await?;
            let legacy_rows = cached_rows.as_ref().is_some_and(|rows| rows.iter().any(|row|
                !row["scanned"].is_boolean() || row["path"].as_str().is_some_and(|path|
                    std::path::Path::new(path).extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("flac")))
            ));
            if legacy_rows || saved.as_ref().and_then(Value::as_str) != Some(current.as_str()) {
                let indexed = actions::files(db, root).await?;
                let rows = actions::cached_mqa_rows(db, &indexed).await?;
                db.set_preference(&format!("desktop-mqa:{root}"), &json!(rows)).await?;
                db.set_preference(&format!("desktop-mqa-manifest:{root}"), &json!(current)).await?;
                cached_rows = Some(rows);
            }
        }
        if route == "local"
            && cached_rows.as_ref().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row.get("date").is_none() || row.get("children").is_none())
            })
        {
            let root = args["root"].as_str().unwrap_or("");
            let indexed = actions::files(db, root).await?;
            let mut rows = cached_rows.take().unwrap_or_default();
            if rows.iter().any(|row| row.get("children").is_none()) {
                rows = duplicates::clusters_to_group_rows(&duplicates::find_duplicate_clusters(
                    &indexed,
                ));
            } else {
                let dates: HashMap<String, String> = indexed
                    .iter()
                    .filter_map(|file| {
                        let tags = workflows::extract_tags_map(&file.metadata);
                        Some((
                            duplicates::extract_release_folder(&file.path),
                            tags.get("date")?.clone(),
                        ))
                    })
                    .collect();
                for row in &mut rows {
                    row["date"] = json!(row["path"]
                        .as_str()
                        .and_then(|path| dates.get(path))
                        .cloned()
                        .unwrap_or_default());
                    if let Some(children) = row["children"].as_array_mut() {
                        for child in children {
                            child["date"] = json!(child["path"]
                                .as_str()
                                .and_then(|path| dates.get(path))
                                .cloned()
                                .unwrap_or_default());
                        }
                    }
                }
            }
            db.set_preference(&format!("desktop-local:{root}"), &json!(rows))
                .await?;
            cached_rows = Some(rows);
        }
        if let Some(mut rows) = cached_rows {
            if filter == Some("affected") {
                rows.retain(|r| r["affected"] == true);
            }
            if let Some(q) = search.filter(|s| !s.is_empty()) {
                let q = q.to_lowercase();
                rows.retain(|r| r.to_string().to_lowercase().contains(&q));
            }
            let sort = sort.unwrap_or("artist");
            rows.sort_by(|a, b| compare_table_cell(a, b, sort));
            if direction == Some("desc") {
                rows.reverse();
            }
            let total = rows.len();
            return Ok(
                json!({"rows":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,"offset":offset,"revision":db.revision.load(Ordering::SeqCst),"preview_id":preview.as_ref().map(|p|p["id"].clone()),"scanned":route == "local" && (local_manifest_saved.is_some() || total > 0)}),
            );
        }
        if route == "queue" || route == "downloaded" {
            let page = db
                .get_queue_rows(route, filter, search, sort, direction, offset, limit)
                .await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }
        if route == "artists" {
            let root = args.get("root").and_then(|v| v.as_str());
            let page = db
                .get_artist_rows(root, filter, search, sort, direction, offset, limit)
                .await?;
            let page = serde_json::to_value(page).map_err(|e| e.to_string())?;
            return if args["local_releases"] == true { artist_release_page(db, args, page).await } else { Ok(page) };
        }
        if route == "favourites" {
            let local_releases = args["local_releases"] == true;
            let page = db
                .get_favourite_rows(
                    args.get("root").and_then(Value::as_str),
                    if local_releases { "all" } else { filter.unwrap_or("all") },
                    if local_releases { "" } else { search.unwrap_or("") },
                    sort.unwrap_or("artist"),
                    direction.unwrap_or("asc"),
                    offset,
                    limit,
                )
                .await?;
            let page = serde_json::to_value(page).map_err(|e| e.to_string())?;
            return if local_releases { Ok(filter_favourite_presentation(artist_release_page(db, args, page).await?, args)) } else { Ok(page) };
        }
        if route == "correct" || route == "organise" {
            let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
            let root_opt = args.get("root").and_then(|v| v.as_str());
            let root = if let Some(r) = root_opt {
                r.to_string()
            } else {
                let roots = db.list_roots(market).await?;
                roots.into_iter().next().map(|r| r.root).unwrap_or_default()
            };
            let action =
                args.get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or(if route == "correct" {
                        "dates"
                    } else {
                        "organise"
                    });
            let files = actions::files(db, &root).await?;
            let settings = db.get_settings().await.unwrap_or(json!({}));
            let template_str = settings
                .get("organisation")
                .and_then(|v| v.get("template"))
                .and_then(|v| v.as_str());
            let plans = workflows::plan_cached(db, &files, action, template_str).await?;
            let file_index: HashMap<_, _> = files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect();
            let preview_id = uuid::Uuid::new_v4().to_string();
            let mut rows: Vec<Value> = plans.iter().filter_map(|plan| {
                let file = file_index.get(plan.path.as_str())?;
                let description = if plan.target.is_some() { "Move or rename file to match its tags" } else { "Standardise local tags" };
                Some(json!({"id":plan.path,"path":plan.path,"artist":plan.artist,"release":plan.album,"title":plan.title,
                    "tags":plan.current_tags,"changes":plan.changes,"target":plan.target,"folder_operation":crate::organisation::folder_operation(&plan.path, plan.target.as_deref()),"evidence":if plan.issues.is_empty() { description.to_string() } else { plan.issues.join("; ") },
                    "affected":!plan.changes.is_empty() || plan.target.is_some(),"status":if plan.changes.is_empty() && plan.target.is_none() { "Needs review" } else { "Needs update" },"size":file.size,"mtime":file.mtime,
                    "item":{"path":plan.path,"target":plan.target,"tags":plan.changes}}))
            }).collect();
            let affected_paths: std::collections::HashSet<String> =
                plans.iter().map(|plan| plan.path.clone()).collect();
            for file in &files {
                if affected_paths.contains(&file.path) {
                    continue;
                }
                let tags = workflows::extract_tags_map(&file.metadata);
                rows.push(json!({"id":file.path,"path":file.path,"artist":tags.get("albumartist").or(tags.get("artist")),
                    "release":tags.get("album"),"title":tags.get("title"),"tags":tags,"changes":{},"target":null,"folder_operation":"No change",
                    "affected":false,"status":"No change","evidence":"No changes needed for this operation"}));
            }
            let source=preview_inputs(db,action).await?;
            {
                let mut previews=state.previews.lock().unwrap();
                previews.retain(|_,p|p["root"]!=root || p["operation"]!=action);
                previews.insert(preview_id.clone(), json!({"id":preview_id,
                    "created":chrono::Utc::now().timestamp_millis(),"operation":action,"root":root,"rows":rows,"count":plans.len(),"source_inputs":source}));
            }
            if filter == Some("affected") {
                rows.retain(|row| row["affected"] == true);
            }
            if let Some(q) = search.filter(|q| !q.is_empty()) {
                let query = q.to_lowercase();
                rows.retain(|row| row.to_string().to_lowercase().contains(&query));
            }
            let key = sort.unwrap_or("artist");
            rows.sort_by(|a, b| compare_table_cell(a, b, key));
            if direction == Some("desc") {
                rows.reverse();
            }
            let total = rows.len();
            return Ok(
                json!({"rows":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,
                "offset":offset,"revision":db.revision.load(Ordering::SeqCst),"preview_id":preview_id}),
            );
        }

        if route == "metadata" || route == "artwork" || route == "mqa" {
            let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
            let root_opt = args.get("root").and_then(|v| v.as_str());
            let root = if let Some(r) = root_opt {
                r.to_string()
            } else {
                let roots = db.list_roots(market).await?;
                roots.into_iter().next().map(|r| r.root).unwrap_or_default()
            };
            if root.is_empty() {
                return Ok(json!({
                    "rows": [],
                    "total": 0,
                    "offset": 0,
                    "revision": 0,
                    "preview_id": null
                }));
            }
            let page = db
                .get_link_rows(
                    market, &root, filter, search, sort, direction, offset, limit,
                )
                .await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }

        if route == "local" {
            let market = args.get("market").and_then(|v| v.as_str()).unwrap_or("GB");
            let root_opt = args.get("root").and_then(|v| v.as_str());
            let root = if let Some(r) = root_opt {
                r.to_string()
            } else {
                let roots = db.list_roots(market).await?;
                roots.into_iter().next().map(|r| r.root).unwrap_or_default()
            };
            if root.is_empty() {
                return Ok(json!({
                    "rows": [],
                    "total": 0,
                    "offset": 0,
                    "revision": 0,
                    "preview_id": null
                }));
            }
            let files = actions::files(db, &root).await?;
            let clusters = duplicates::find_duplicate_clusters(&files);
            let mut rows = duplicates::clusters_to_link_rows(&clusters);

            if let Some(q) = search {
                let q_lower = q.to_lowercase();
                rows.retain(|r| {
                    r.artist.to_lowercase().contains(&q_lower)
                        || r.release.to_lowercase().contains(&q_lower)
                        || r.target.to_lowercase().contains(&q_lower)
                        || r.evidence.to_lowercase().contains(&q_lower)
                });
            }

            if let Some(key) = sort {
                rows.sort_by(|a, b| {
                    let left = serde_json::to_value(a).unwrap_or(Value::Null);
                    let right = serde_json::to_value(b).unwrap_or(Value::Null);
                    let order = compare_table_cell(&left, &right, key);
                    if direction == Some("desc") {
                        order.reverse()
                    } else {
                        order
                    }
                });
            }
            let total = rows.len();
            let page_rows = rows.into_iter().skip(offset).take(limit).collect();
            return serde_json::to_value(crate::db::TablePage {
                rows: page_rows,
                total,
                offset,
                revision: 0,
                preview_id: Some("local_duplicates".to_string()),
            })
            .map_err(|e| e.to_string());
        }

        return Ok(json!({
            "rows": [],
            "total": 0,
            "offset": 0,
            "revision": 0,
            "preview_id": null
        }));
    }

}

pub async fn table_result(state: &Arc<Backend>, db: &TursoDb, args: &Value, is_facet: bool) -> Result<Value, String> {
    let filters = parse(args.get("column_filters"))?;
    let grouped_links = args["route"] == "links" && args["group_releases"] == true;
    let normalize = |mut page: Value| {
        if let Some(rows) = page["rows"].as_array_mut() {
            // Before a metadata/artwork preview exists, its fallback rows are
            // linked files, not proposed changes. Apply the scope consistently.
            if args["filter"] == "affected" { rows.retain(|row| row["affected"] == true); }
            for row in rows.iter_mut().filter(|row| row["ignored"] == true) { row["status"] = json!("Ignored"); }
            if let Some(key) = args["sort"].as_str() {
                let descending = args["direction"] == "desc" ||
                    (args["direction"].is_null() && args["route"] == "missing" && key == "date");
                rows.sort_by(|left,right| {
                    let order = compare_table_cell(left,right,key);
                    (if descending { order.reverse() } else { order })
                        .then_with(|| compare_table_cell(left,right,"id"))
                });
            }
        }
        page
    };
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    let column = if is_facet { Some(args["column"].as_str().filter(|column| !column.is_empty())
        .ok_or("Choose a table column")?) } else { None };

    let mut source_args = args.clone();
    if let Some(source) = source_args.as_object_mut() {
        for key in ["column_filters", "column", "facet_search", "group_releases", "group_artists"] { source.remove(key); }
        source.insert("offset".into(), json!(0));
        source.insert("limit".into(), json!(usize::MAX));
    }
    // Filtering/paging one column must not repeatedly reload a large catalogue.
    // This uses the existing per-view cache and invalidation, independently of
    // workers and the visible table request. No online requests are made here.
    let key = format!("table.source:{source_args}");
    let gate = state.read_gate(&key);
    let _source_read = gate.lock().await;
    let revision = db.revision.load(Ordering::SeqCst);
    let cached = state.view_cache.lock().unwrap().get(&key)
        .filter(|(saved, at, _)| *saved == revision && at.elapsed().as_secs() < 30)
        .map(|(_, _, page)| page.clone());
    let page = if let Some(page) = cached { page } else {
        let page = normalize(get_table_page(state, db, &source_args).await?);
        if db.revision.load(Ordering::SeqCst) == revision {
            let mut cache = state.view_cache.lock().unwrap();
            if cache.len() >= 64 { cache.clear(); }
            cache.insert(key,(revision,std::time::Instant::now(),page.clone()));
        }
        page
    };
    Ok(if grouped_links {
        if let Some(column) = column {
            grouped_link_facets(&page,&filters,args,column,args["facet_search"].as_str().unwrap_or(""),offset,limit.min(500))
        } else {
            grouped_link_page(page,&filters,args,offset,limit)
        }
    } else if let Some(column) = column {
        facets(&page,&filters,column,args["facet_search"].as_str().unwrap_or(""),offset,limit.min(500))
    } else {
        filter_page(page,&filters,offset,limit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_row(path: &str, position: &str, status: &str) -> Value {
        json!({"id":path,"path":path,"artist":"Artist","release":"Release","title":"Track",
            "position":position,"status":status,"evidence":"Exact recording","ignored":status == "Ignored"})
    }

    #[test]
    fn link_groups_merge_discs_but_keep_separate_release_folders_and_original_children() {
        let later = link_row("/Music/Artist/Release/Disc 2/01.flac", "Disc 02/02 · Track 01/01", "Needs choice");
        let first = link_row("/Music/Artist/Release/Disc 1/01.flac", "Disc 01/02 · Track 01/01", "Linked");
        let other = link_row("/Music/Artist/Release deluxe/01.flac", "Disc 01/01 · Track 01/01", "Unlinked");
        let page = json!({"rows":[later,first,other],"revision":4});
        let groups = link_release_groups(&page, &ColumnFilters::new(), None);
        assert_eq!(groups.len(), 2, "the local edition folder is part of the identity");
        let release = groups.iter().find(|group| group["path"] == "/Music/Artist/Release").unwrap();
        assert_eq!(release["tracks"], 2);
        assert_eq!(release["status"], "Needs choice");
        assert_eq!(release["evidence"], "2 tracks · 1 linked · 1 needs choice");
        assert_eq!(release["children"], json!([first,later]), "file-action data stays unchanged");
        assert_eq!(release["track_ids"], json!([first["id"],later["id"]]));
        let filtered = parse(Some(&json!({"status":{"include":["Needs choice"]}}))).unwrap();
        let filtered_groups = link_release_groups(&page, &filtered, None);
        assert_eq!(filtered_groups[0]["id"], release["id"], "filtering does not change an expansion identity");
    }

    #[test]
    fn link_groups_filter_tracks_before_paging_whole_releases() {
        let page = json!({"rows":[
            link_row("/Music/Artist/A/01.flac", "Disc 01/01 · Track 01/03", "Linked"),
            link_row("/Music/Artist/A/02.flac", "Disc 01/01 · Track 02/03", "Unlinked"),
            link_row("/Music/Artist/A/03.flac", "Disc 01/01 · Track 03/03", "Unlinked"),
            link_row("/Music/Artist/B/01.flac", "Disc 01/01 · Track 01/02", "Linked"),
            link_row("/Music/Artist/B/02.flac", "Disc 01/01 · Track 02/02", "Unlinked")
        ],"revision":7,"preview_id":null});
        let filters = parse(Some(&json!({"status":{"exclude":["Linked"]}}))).unwrap();
        let args = json!({"sort":"tracks","direction":"desc"});
        let first = grouped_link_page(page.clone(), &filters, &args, 0, 1);
        assert_eq!(first["total"], 2);
        assert_eq!(first["track_total"], 3);
        assert_eq!(first["rows"][0]["children"].as_array().unwrap().len(), 2);
        assert!(first["rows"][0]["children"].as_array().unwrap().iter().all(|child| child["status"] == "Unlinked"));
        let second = grouped_link_page(page, &filters, &args, 1, 1);
        assert_eq!(second["rows"][0]["tracks"], 1);
        assert_eq!(second["offset"], 1);
        assert_eq!(second["revision"], 7);
    }

    #[test]
    fn link_groups_sort_track_counts_and_disc_track_indices_numerically() {
        let mut rows: Vec<_> = (1..=12).rev().map(|track| link_row(
            &format!("/Music/Artist/Large/{track}.flac"),
            &format!("Disc 1/1 · Track {track}/12"), "Linked")).collect();
        rows.push(link_row("/Music/Artist/Small/CD 10/01.flac", "Disc 10/10 · Track 1/1", "Linked"));
        rows.push(link_row("/Music/Artist/Small/CD 2/01.flac", "Disc 2/10 · Track 1/1", "Linked"));
        let page = json!({"rows":rows});
        let result = grouped_link_page(page.clone(), &ColumnFilters::new(), &json!({"sort":"tracks"}), 0, 10);
        assert_eq!(result["rows"][0]["tracks"], 2);
        assert_eq!(result["rows"][0]["children"][0]["position"], "Disc 2/10 · Track 1/1");
        assert_eq!(result["rows"][1]["children"][1]["position"], "Disc 1/1 · Track 2/12");
        let reversed = grouped_link_page(page, &ColumnFilters::new(), &json!({"sort":"tracks","direction":"desc"}), 0, 10);
        assert_eq!(reversed["rows"][0]["tracks"], 12);
    }

    #[test]
    fn link_facets_keep_track_statuses_and_honor_release_count_filters() {
        let page = json!({"rows":[
            link_row("/Music/Artist/A/01.flac", "Disc 01/01 · Track 01/02", "Unlinked"),
            link_row("/Music/Artist/A/02.flac", "Disc 01/01 · Track 02/02", "Needs choice"),
            link_row("/Music/Artist/B/01.flac", "Disc 01/01 · Track 01/01", "Linked")
        ],"revision":9});
        let filters = parse(Some(&json!({"tracks":{"include":["2"]},"status":{"exclude":["Linked"]}}))).unwrap();
        let statuses = grouped_link_facets(&page, &filters, &json!({}), "status", "", 0, 100);
        assert_eq!(statuses["total"], 2);
        assert!(statuses["options"].as_array().unwrap().iter().all(|option| option["value"] != "Linked"));
        let counts = grouped_link_facets(&page, &filters, &json!({}), "tracks", "", 0, 100);
        assert_eq!(counts["options"], json!([{"value":"2","label":"2","count":1}]));
        let only_small = parse(Some(&json!({"tracks":{"include":["1"]}}))).unwrap();
        let small = grouped_link_page(page, &only_small, &json!({}), 0, 10);
        assert_eq!(small["total"], 1);
        assert_eq!(small["track_total"], 1);
        assert_eq!(small["rows"][0]["status"], "Linked");
    }

    #[test]
    fn artist_link_hierarchy_groups_whole_artists_and_keeps_track_action_ids() {
        let first = link_row("/Music/Artist/A/Disc 1/01.flac", "Disc 1/2 · Track 1/1", "Linked");
        let second = link_row("/Music/Artist/A/Disc 2/01.flac", "Disc 2/2 · Track 1/1", "Unlinked");
        let third = link_row("/Music/Artist/B/01.flac", "Disc 1/1 · Track 1/1", "Needs choice");
        let mut other = link_row("/Music/Other/C/01.flac", "Disc 1/1 · Track 1/1", "Linked");
        other["artist"] = json!("Other");
        let page = json!({"rows":[first.clone(),second.clone(),third.clone(),other],"root":"/Music/","revision":10});
        let args = json!({"group_artists":true,"sort":"tracks","direction":"desc"});
        let result = grouped_link_page(page.clone(), &ColumnFilters::new(), &args, 0, 1);
        assert_eq!(result["total"], 2);
        assert_eq!(result["artist_total"], 2);
        assert_eq!(result["release_total"], 3);
        assert_eq!(result["track_total"], 4);
        let artist = &result["rows"][0];
        assert_eq!(artist["artist_group"], true);
        assert_eq!(artist["release"], "2 releases");
        assert_eq!(artist["linked_tracks"], 1);
        assert_eq!(artist["unlinked_tracks"], 1);
        assert_eq!(artist["choice_tracks"], 1);
        assert_eq!(artist["track_ids"], json!([first["id"],second["id"],third["id"]]));
        assert_eq!(artist["children"][0]["link_group"], true);
        assert_eq!(artist["children"][0]["children"], json!([first,second]));
        let filters = parse(Some(&json!({"status":{"include":["Unlinked"]}}))).unwrap();
        let filtered = grouped_link_page(page, &filters, &args, 0, 100);
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["release_total"], 1);
        assert_eq!(filtered["track_total"], 1);
        assert_eq!(filtered["rows"][0]["id"], artist["id"], "filter changes do not move the expansion identity");
    }

    #[test]
    fn compilation_releases_remain_under_one_artist_without_performer_leakage() {
        let mut first = link_row("/Music/Compilations/A/01.flac", "Disc 1/1 · Track 1/2", "Linked");
        first["artist"] = json!("Various Artists");
        first["performer"] = json!("Singer A");
        let mut second = link_row("/Music/Compilations/A/02.flac", "Disc 1/1 · Track 2/2", "Unlinked");
        second["artist"] = json!("V.A.");
        second["performer"] = json!("Singer B");
        let result = grouped_link_page(json!({"rows":[first,second],"root":"/Music"}),
            &ColumnFilters::new(), &json!({"group_artists":true}), 0, 100);
        assert_eq!(result["total"], 1);
        assert_eq!(result["release_total"], 1);
        assert_eq!(result["rows"][0]["artist"], "Various Artists");
        assert_eq!(result["rows"][0]["tracks"], 2);
        assert_eq!(result["rows"][0]["children"][0]["children"][1]["performer"], "Singer B");
    }

    #[test]
    fn compilation_artist_filters_and_facets_share_the_canonical_group_label() {
        let mut first = link_row("/Music/Compilations/A/01.flac", "Disc 1/1 · Track 1/2", "Linked");
        first["artist"] = json!("Various Artists");
        let mut second = link_row("/Music/Compilations/A/02.flac", "Disc 1/1 · Track 2/2", "Unlinked");
        second["artist"] = json!("V.A.");
        let page = json!({"rows":[first,second,link_row("/Music/Artist/A/01.flac", "Disc 1/1 · Track 1/1", "Linked")]});
        let args = json!({"group_artists":true});
        let filters = parse(Some(&json!({"artist":{"include":["Various Artists"]}}))).unwrap();
        let result = grouped_link_page(page.clone(), &filters, &args, 0, 100);
        assert_eq!(result["track_total"], 2);
        assert_eq!(result["rows"][0]["children"][0]["children"][1]["artist"], "V.A.", "original file tags are preserved");
        let facet = grouped_link_facets(&page, &filters, &args, "artist", "various", 0, 100);
        assert_eq!(facet["options"], json!([{"value":"Various Artists","label":"Various Artists","count":2}]));
        let old_alias_filter = parse(Some(&json!({"artist":{"include":["V.A."]}}))).unwrap();
        assert_eq!(grouped_link_page(page.clone(), &old_alias_filter, &args, 0, 100)["track_total"], 2);
        let excluded = parse(Some(&json!({"artist":{"exclude":["Various Artists"]}}))).unwrap();
        assert_eq!(grouped_link_page(page, &excluded, &args, 0, 100)["track_total"], 1);
    }

    #[test]
    fn artist_link_facets_use_actual_track_statuses_and_artist_aggregate_counts() {
        let mut second = link_row("/Music/Other/C/01.flac", "Disc 1/1 · Track 1/1", "Linked");
        second["artist"] = json!("Other");
        let page = json!({"rows":[
            link_row("/Music/Artist/A/01.flac", "Disc 1/1 · Track 1/1", "Linked"),
            link_row("/Music/Artist/B/01.flac", "Disc 1/1 · Track 1/1", "Unlinked"), second]});
        let args = json!({"group_artists":true});
        let filters = parse(Some(&json!({"tracks":{"include":["2"]}}))).unwrap();
        let counts = grouped_link_facets(&page, &filters, &args, "tracks", "", 0, 100);
        assert_eq!(counts["options"], json!([{"value":"1","label":"1","count":1},{"value":"2","label":"2","count":1}]));
        let statuses = grouped_link_facets(&page, &filters, &args, "status", "", 0, 100);
        assert_eq!(statuses["options"], json!([{"value":"Linked","label":"Linked","count":1},{"value":"Unlinked","label":"Unlinked","count":1}]));
        let result = grouped_link_page(page, &filters, &args, 0, 100);
        assert_eq!(result["total"], 1);
        assert_eq!(result["release_total"], 2);
        let mut release_rows = vec![];
        for (artist, count) in [("Smaller",2),("Larger",10)] {
            for index in 0..count {
                let mut row = link_row(&format!("/Music/{artist}/{index}/01.flac"), "Disc 1/1 · Track 1/1", "Linked");
                row["artist"] = json!(artist);
                release_rows.push(row);
            }
        }
        let sorted = grouped_link_page(json!({"rows":release_rows}), &ColumnFilters::new(),
            &json!({"group_artists":true,"sort":"release"}), 0, 100);
        assert_eq!(sorted["rows"][0]["releases"], 2, "displayed release counts sort numerically");
        assert_eq!(sorted["rows"][1]["releases"], 10);
    }

    #[test]
    fn local_artist_release_summaries_show_catalogue_links_without_track_expansion() {
        let linked = link_row("/Music/Artist/A/01.flac", "Disc 1/1 · Track 1/2", "Linked");
        let unlinked = link_row("/Music/Artist/A/02.flac", "Disc 1/1 · Track 2/2", "Unlinked");
        let releases = link_release_groups(&json!({"rows":[linked,unlinked]}), &ColumnFilters::new(), None);
        let mappings = vec![json!({"id":"Artist","artist":"Artist","resolved":true,"online_id":"123"})];
        let dates = HashMap::from([("/Music/Artist/A".into(), "2024-05-01".into())]);
        let page = decorate_local_releases(json!({"rows":[
            {"id":"local:artist","artist":"Artist","status":"Local only","tracks":2,"online_id":""},
            {"id":"favourite:456","artist":"Remote only","status":"Missing locally","tracks":0,"online_id":"456"}
        ]}), releases, &mappings, &dates, true);
        let artist = &page["rows"][0];
        assert_eq!(artist["id"], "local:artist", "artist actions retain their existing identity");
        assert_eq!(artist["status"], "Local only", "favourite state and catalogue linkage are independent");
        assert_eq!(artist["catalogue_status"], "Linked");
        assert_eq!(artist["online_id"], "123");
        assert_eq!(artist["release"], 1);
        let release = &artist["children"][0];
        assert_eq!(release["row_kind"], "artist_release");
        assert_eq!(release["parent_id"], "local:artist");
        assert_eq!(release["link_group"], false);
        assert_eq!(release["local_release"], true);
        assert_eq!(release["date"], "2024-05-01");
        assert_eq!(release["linked_tracks"], 1);
        assert_eq!(release["unlinked_tracks"], 1);
        assert!(release.get("children").is_none());
        assert!(release.get("track_ids").is_none());
        assert_eq!(page["rows"][1]["catalogue_status"], "Not linked");
        assert_eq!(page["rows"][1]["expanded_available"], false);
    }

    #[test]
    fn favourite_confirmed_ids_unify_local_aliases_before_status_and_search_filters() {
        let mut first = link_row("/Music/Local Artist/A/01.flac", "Disc 1/1 · Track 1/1", "Linked");
        first["artist"] = json!("Local Artist");
        let mut second = link_row("/Music/Other Alias/B/01.flac", "Disc 1/1 · Track 1/1", "Unlinked");
        second["artist"] = json!("Other Alias");
        let releases = link_release_groups(&json!({"rows":[first,second]}), &ColumnFilters::new(), None);
        let mappings = vec![
            json!({"artist":"Local Artist","resolved":true,"online_id":"123"}),
            json!({"artist":"Other Alias","resolved":true,"online_id":"123"}),
        ];
        let page = decorate_local_releases(json!({"rows":[
            {"id":"favourite:123","artist":"Online Display Name","status":"Missing locally","tracks":0,"online_id":"123"},
            {"id":"local:local artist","artist":"Local Artist","status":"Local only","tracks":1,"online_id":""},
            {"id":"local:other alias","artist":"Other Alias","status":"Local only","tracks":1,"online_id":""}
        ]}), releases.clone(), &mappings, &HashMap::new(), true);
        assert_eq!(page["total"], 1);
        let artist = &page["rows"][0];
        assert_eq!(artist["id"], "favourite:123");
        assert_eq!(artist["artist"], "Online Display Name");
        assert_eq!(artist["lookup_artist"], "Local Artist");
        assert_eq!(artist["local_aliases"], json!(["Local Artist","Other Alias"]));
        assert_eq!(artist["status"], "In library");
        assert_eq!(artist["catalogue_status"], "Linked");
        assert_eq!(artist["tracks"], 2);
        assert_eq!(artist["releases"], 2);
        assert!(artist["children"].as_array().unwrap().iter().all(|release|release["parent_id"] == "favourite:123"));
        let searched = filter_favourite_presentation(page.clone(), &json!({"filter":"In library","search":"Other Alias"}));
        assert_eq!(searched["total"], 1, "local aliases remain searchable after reconciliation");
        assert_eq!(filter_favourite_presentation(page, &json!({"filter":"Local only"}))["total"], 0);
        let unresolved = decorate_local_releases(json!({"rows":[
            {"id":"favourite:123","artist":"Online Display Name","status":"Missing locally","tracks":0,"online_id":"123"},
            {"id":"local:local artist","artist":"Local Artist","status":"Local only","tracks":1,"online_id":""}
        ]}), releases, &[json!({"artist":"Local Artist","resolved":false,"online_id":"123"})], &HashMap::new(), true);
        assert_eq!(unresolved["total"], 2, "unconfirmed candidates must not merge identities");
        assert_eq!(unresolved["rows"][0]["status"], "Missing locally");
        assert_eq!(unresolved["rows"][0]["catalogue_status"], "Not linked");
    }

    #[test]
    fn filters_use_or_within_columns_and_and_between_columns_before_paging() {
        let page = json!({"rows":[
            {"id":"a","artist":"One","status":"Linked","tracks":2},
            {"id":"b","artist":"Two","status":"Needs review","tracks":4},
            {"id":"c","artist":"Three","status":"Needs review","tracks":5},
            {"id":"d","artist":"Two","status":"Linked","tracks":9}
        ],"total":4,"offset":0,"revision":3,"preview_id":"keep"});
        let filters = parse(Some(&json!({"artist":{"include":["One","Two"]},"status":{"exclude":["Linked"]}}))).unwrap();
        let filtered = filter_page(page.clone(),&filters,0,1);
        assert_eq!(filtered["total"],1);
        assert_eq!(filtered["rows"][0]["id"],"b");
        assert_eq!(filtered["preview_id"],"keep");
        let facets = facets(&page,&filters,"status","",0,100);
        assert_eq!(facets["total"],2,"the open column does not hide its unchecked values");
        assert_eq!(facets["options"][0],json!({"value":"Linked","label":"Linked","count":2}));
        assert_eq!(facets["options"][1]["count"],1);
        let none = parse(Some(&json!({"artist":{"include":[]}}))).unwrap();
        assert_eq!(filter_page(page,&none,0,10)["total"],0);
    }

    #[test]
    fn facet_search_pages_values_and_preserves_readable_types() {
        let page = json!({"rows":[{"tracks":12},{"tracks":2},{"tracks":2},{"tracks":null}],"revision":8});
        let result = facets(&page,&ColumnFilters::new(),"tracks","",1,1);
        assert_eq!(result["total"],3);
        assert_eq!(result["options"][0]["value"],"12");
        assert_eq!(result["revision"],8);
        assert_eq!(facets(&page,&ColumnFilters::new(),"tracks","2",0,100)["total"],2);
        assert_eq!(readable(&json!({"new_value":["a",2,true,null]})),"new value: a · 2 · true · —");
        assert!(parse(Some(&json!({"status":{"include":[1]}}))).is_err());
        assert!(parse(Some(&json!([]))).is_err());
    }
}
