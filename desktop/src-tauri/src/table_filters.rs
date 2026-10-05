//! Shared table filtering, including facets over the complete scoped dataset.
//! Values match the frontend's `readable` formatter, including missing cells.
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashMap, HashSet}, sync::{Arc, atomic::Ordering}};
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
        for key in ["column_filters", "column", "facet_search"] { source.remove(key); }
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
    Ok(if let Some(column) = column {
        facets(&page,&filters,column,args["facet_search"].as_str().unwrap_or(""),offset,limit.min(500))
    } else {
        filter_page(page,&filters,offset,limit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
