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
        groups.entry((folder, canonical("artist"), canonical("release")))
            .or_default().push(row.clone());
    }
    groups.into_iter().map(|(key, mut children)| {
        children.sort_by(|left, right| link_position(left).cmp(&link_position(right))
            .then_with(|| compare_table_cell(left, right, "id")));
        let count = children.len();
        let mut linked = 0;
        let mut choices = 0;
        let mut ignored = 0;
        let mut unlinked = 0;
        for child in &children {
            match child["status"].as_str().unwrap_or("") {
                "Ignored" => ignored += 1,
                "Linked" => linked += 1,
                "Needs choice" => choices += 1,
                _ => unlinked += 1,
            }
        }
        let status = if ignored == count { "Ignored" }
            else if choices > 0 { "Needs choice" }
            else if unlinked > 0 { "Unlinked" }
            else { "Linked" };
        let mut summary = vec![format!("{count} {}", if count == 1 { "track" } else { "tracks" })];
        for (number, label) in [(linked, "linked"), (choices, if choices == 1 { "needs choice" } else { "need choice" }),
            (unlinked, "unlinked"), (ignored, "ignored")]
        {
            if number > 0 { summary.push(format!("{number} {label}")); }
        }
        let identity = serde_json::to_vec(&key).expect("release key serializes");
        let id = format!("local-release:{:x}", Sha256::digest(identity));
        let track_ids: Vec<_> = children.iter().map(|child| child["id"].clone()).collect();
        json!({"id":id,"artist":children[0]["artist"],"release":children[0]["release"],
            "tracks":count,"status":status,"evidence":summary.join(" · "),"path":key.0,
            "track_ids":track_ids,"children":children,"expanded_available":true,"link_group":true,
            "ignored":ignored == count})
    }).collect()
}

fn grouped_link_page(mut page: Value, filters: &ColumnFilters, args: &Value, offset: usize, limit: usize) -> Value {
    let mut rows = link_release_groups(&page, filters, None);
    rows.retain(|row| matches_link_count(row, filters));
    let key = args["sort"].as_str().unwrap_or("artist");
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
    page["track_total"] = json!(rows.iter().map(|row| row["tracks"].as_u64().unwrap_or(0)).sum::<u64>());
    page["offset"] = json!(offset);
    page["rows"] = json!(rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>());
    page
}

fn grouped_link_facets(page: &Value, filters: &ColumnFilters, column: &str, search: &str, offset: usize, limit: usize) -> Value {
    let mut groups = link_release_groups(page, filters, Some(column));
    if column != "tracks" { groups.retain(|row| matches_link_count(row, filters)); }
    // Counts are a release-level column. All other values and counts continue
    // to describe the matching tracks, including different statuses in one release.
    let rows = if column == "tracks" { groups } else {
        groups.into_iter().flat_map(|mut group| group["children"].as_array_mut()
            .map(std::mem::take).unwrap_or_default()).collect()
    };
    facets(&json!({"rows":rows,"revision":page["revision"]}), &ColumnFilters::new(), column, search, offset, limit)
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
        return serde_json::to_value(page).map_err(|e| e.to_string());
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
            return serde_json::to_value(page).map_err(|e| e.to_string());
        }
        if route == "favourites" {
            let page = db
                .get_favourite_rows(
                    args.get("root").and_then(Value::as_str),
                    filter.unwrap_or("all"),
                    search.unwrap_or(""),
                    sort.unwrap_or("artist"),
                    direction.unwrap_or("asc"),
                    offset,
                    limit,
                )
                .await?;
            return serde_json::to_value(page).map_err(|e| e.to_string());
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
        for key in ["column_filters", "column", "facet_search", "group_releases"] { source.remove(key); }
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
            grouped_link_facets(&page,&filters,column,args["facet_search"].as_str().unwrap_or(""),offset,limit.min(500))
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
        let statuses = grouped_link_facets(&page, &filters, "status", "", 0, 100);
        assert_eq!(statuses["total"], 2);
        assert!(statuses["options"].as_array().unwrap().iter().all(|option| option["value"] != "Linked"));
        let counts = grouped_link_facets(&page, &filters, "tracks", "", 0, 100);
        assert_eq!(counts["options"], json!([{"value":"2","label":"2","count":1}]));
        let only_small = parse(Some(&json!({"tracks":{"include":["1"]}}))).unwrap();
        let small = grouped_link_page(page, &only_small, &json!({}), 0, 10);
        assert_eq!(small["total"], 1);
        assert_eq!(small["track_total"], 1);
        assert_eq!(small["rows"][0]["status"], "Linked");
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
