//! Cache-only pipeline checks for sibling-supported, same-release linking.

use crate::db::TursoDb;
use crate::linking::link_library_mode;
use crate::tidal::{TidalCatalogue, TidalRelease, TidalTrack};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const ROOT: &str = "/synthetic-release-context";
const ARTIST: &str = "Context Orchestra";
const ALBUM: &str = "Last Light";
const ALBUM_ID: &str = "810001";

async fn database() -> (TursoDb, PathBuf) {
    let directory = std::env::temp_dir().join(format!("release-context-{}", uuid::Uuid::new_v4()));
    let db = TursoDb::open(directory.join("test.sqlite3")).await.unwrap();
    (db, directory)
}

fn release(disc_totals: &[u32]) -> TidalRelease {
    let mut tracks = Vec::new();
    for (disc, count) in disc_totals.iter().enumerate() {
        for number in 1..=*count {
            let sequence = tracks.len() + 1;
            tracks.push(TidalTrack {
                id: format!("810001{sequence:02}"),
                title: format!("Recording {sequence}"),
                isrc: Some(format!("GBTEST260{sequence:03}")),
                duration: 180.0 + sequence as f64,
                disc_number: disc as u32 + 1,
                track_number: number,
                ..Default::default()
            });
        }
    }
    TidalRelease {
        id: ALBUM_ID.into(),
        artist: ARTIST.into(),
        title: ALBUM.into(),
        r#type: "album".into(),
        tracks_loaded: true,
        track_count: tracks.len(),
        available: Some(true),
        tracks,
        ..Default::default()
    }
}

async fn cache(db: &TursoDb, releases: Vec<TidalRelease>) {
    let connection = db.connect().unwrap();
    connection
        .execute(
            "INSERT INTO mappings(artist,tidal_id,status) VALUES(?, '800001', 'confirmed')",
            (ARTIST,),
        )
        .await
        .unwrap();
    for release in &releases {
        db.set_preference(&format!("tag-review:GB:{}", release.id), &json!(release))
            .await
            .unwrap();
        db.set_preference(
            &format!("release-live:GB:{}", release.id),
            &json!({"available":true,"checked_at":chrono::Utc::now().timestamp(),"source":"album_lookup"}),
        )
        .await
        .unwrap();
    }
    let catalogue = TidalCatalogue {
        id: "800001".into(),
        name: ARTIST.into(),
        releases,
    };
    connection
        .execute(
            "INSERT INTO catalogue(artist_id,market,payload,fetched) VALUES('800001','GB',?,'2026-10-06')",
            (json!(catalogue).to_string(),),
        )
        .await
        .unwrap();
}

async fn local_files(
    db: &TursoDb,
    release: &TidalRelease,
    track_total: u32,
    conflicting_isrc: Option<usize>,
    differing_duration: Option<usize>,
) -> Vec<String> {
    let disc_total = release
        .tracks
        .iter()
        .map(|track| track.disc_number)
        .max()
        .unwrap();
    let mut paths = Vec::new();
    for (index, track) in release.tracks.iter().enumerate() {
        let disc_folder = if disc_total > 1 {
            format!("/Disc {}", track.disc_number)
        } else {
            String::new()
        };
        let path = format!(
            "{ROOT}/{ARTIST}/{ALBUM}{disc_folder}/{:02}.flac",
            track.track_number
        );
        let metadata = json!({
            "albumartist":ARTIST,"artist":ARTIST,"album":ALBUM,"title":track.title,
            "tracknumber":format!("{:02}/{track_total:02}",track.track_number),
            "discnumber":format!("{:02}/{disc_total:02}",track.disc_number),
            "duration":track.duration + if differing_duration == Some(index) {10.0} else {0.2},
            "isrc":if conflicting_isrc == Some(index) {Some("GBWRONG260004".to_string())} else {track.isrc.clone()},
        });
        db.apply_file_update(&path, &path, ROOT, &metadata, 10, 20)
            .await
            .unwrap();
        paths.push(path);
    }
    paths
}

async fn payload(db: &TursoDb, path: &str) -> Value {
    let connection = db.connect().unwrap();
    let mut rows = connection
        .query(
            "SELECT payload FROM track_links WHERE path=? AND market='GB'",
            (path,),
        )
        .await
        .unwrap();
    let raw: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    serde_json::from_str(&raw).unwrap()
}

async fn link(db: &TursoDb) -> crate::linking::LinkSummary {
    link_library_mode(
        db,
        "GB",
        ROOT,
        Arc::new(AtomicBool::new(false)),
        |_| {},
        None,
        false,
        true,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn omitted_recording_explains_audio_counts_without_creating_a_wrong_placement() {
    let (db, directory) = database().await;
    let complete_local = release(&[11]);
    let mut shortened = complete_local.clone();
    shortened.tracks.remove(8);
    for (index, track) in shortened.tracks.iter_mut().enumerate() {
        track.track_number = index as u32 + 1;
    }
    shortened.track_count = 10;
    shortened.quality = "LOSSLESS".into();
    let mut alternate = shortened.clone();
    alternate.id = "810004".into();
    alternate.quality = "DOLBY_ATMOS".into();
    for (index, track) in alternate.tracks.iter_mut().enumerate() {
        track.id = format!("810004{:02}", index + 1);
    }
    cache(&db, vec![shortened.clone(), alternate]).await;
    // Impossible saved totals must not conceal the eleven physical local files.
    let paths = local_files(&db, &complete_local, 1, None, None).await;
    let mut existing = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if index == 8 {
            continue;
        }
        let remote_index = if index < 8 { index } else { index - 1 };
        let saved = json!({"status":"linked","manual":index == 0,"scope":"track","ids":{"album_id":ALBUM_ID,"track_id":shortened.tracks[remote_index].id}});
        db.save_track_link(path, "GB", "[0,0,10,20]", &saved.to_string())
            .await
            .unwrap();
        existing.push((path.clone(), saved));
    }
    let summary = link(&db).await;
    assert_eq!(
        (
            summary.total,
            summary.linked,
            summary.review,
            summary.unmatched
        ),
        (1, 0, 1, 0)
    );
    let review = payload(&db, &paths[8]).await;
    assert_eq!(review["status"], "review");
    assert!(review["ids"].is_null());
    let note = review["catalogue_note"].as_str().unwrap();
    assert!(note.contains("Local release: 11 tracks · online candidate: 10 audio tracks"));
    assert!(note.contains("recording absent"));
    let options = review["catalogue_options"].as_array().unwrap();
    assert_eq!(
        options.len(),
        2,
        "Both shortened editions remain inspectable release candidates"
    );
    for option in options {
        assert!(option["track_id"].is_null(), "The song occupying the omitted recording's old position must not be offered as its placement");
        assert_eq!(option["audio_tracks"], 10);
        assert_eq!(option["local_tracks"], 11);
        assert_eq!(option["compatible"], false);
        assert_eq!(option["recording_absent"], true);
        assert!(option["evidence"]
            .as_str()
            .unwrap()
            .contains("recording absent"));
    }
    let repeated = link_library_mode(
        &db,
        "GB",
        ROOT,
        Arc::new(AtomicBool::new(false)),
        |_| {},
        None,
        true,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        repeated.total, 0,
        "Inspectable editions without the recording are not available placement choices"
    );
    assert_eq!(payload(&db, &paths[8]).await, review);
    for (path, saved) in existing {
        assert_eq!(
            payload(&db, &path).await,
            saved,
            "Existing sibling choices remain unchanged"
        );
    }
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn wholly_unmatched_recording_keeps_diagnostics_without_creating_choices() {
    let (db, directory) = database().await;
    let remote = release(&[1]);
    cache(&db, vec![remote.clone()]).await;
    let mut local = remote;
    local.tracks[0].title = "Absent Recording".into();
    local.tracks[0].isrc = Some("GBOTHER260001".into());
    let paths = local_files(&db, &local, 1, None, None).await;
    let summary = link(&db).await;
    assert_eq!(
        (summary.linked, summary.review, summary.unmatched),
        (0, 0, 1)
    );
    let saved = payload(&db, &paths[0]).await;
    assert_eq!(saved["status"], "unmatched");
    assert!(saved["catalogue_options"].is_null());
    assert!(saved["catalogue_note"]
        .as_str()
        .unwrap()
        .contains("recording absent"));
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn majority_records_resolve_conflicting_isrc_only_inside_the_same_release() {
    let (db, directory) = database().await;
    let mut album = release(&[4]);
    album.tracks[3].title = ALBUM.into();
    let mut single = album.clone();
    single.id = "810002".into();
    single.r#type = "single".into();
    single.track_count = 1;
    single.tracks = vec![TidalTrack {
        id: "81000201".into(),
        title: ALBUM.into(),
        duration: album.tracks[3].duration,
        isrc: Some("GBWRONG260004".into()),
        track_number: 1,
        disc_number: 1,
        ..Default::default()
    }];
    cache(&db, vec![single, album.clone()]).await;
    let paths = local_files(&db, &album, 4, Some(3), None).await;
    let summary = link(&db).await;
    assert_eq!(
        (
            summary.total,
            summary.linked,
            summary.review,
            summary.unmatched
        ),
        (4, 4, 0, 0)
    );
    for (index, path) in paths.iter().enumerate() {
        let saved = payload(&db, path).await;
        assert_eq!(saved["ids"]["album_id"], ALBUM_ID);
        assert_eq!(saved["ids"]["track_id"], album.tracks[index].id);
        assert!(saved["placements"]
            .as_array()
            .unwrap()
            .iter()
            .all(|placement| placement["album_id"] == ALBUM_ID));
    }
    let reconciled = payload(&db, &paths[3]).await;
    assert_eq!(reconciled["release_context"]["isrc_conflict"], true);
    assert!(reconciled["catalogue_options"][0]["evidence"]
        .as_str()
        .unwrap()
        .contains("ISRC differs"));
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn same_release_keeps_duration_conflict_in_review_and_links_matching_siblings() {
    let (db, directory) = database().await;
    let album = release(&[4]);
    cache(&db, vec![album.clone()]).await;
    let paths = local_files(&db, &album, 4, None, Some(3)).await;
    let manual = json!({"status":"linked","manual":true,"scope":"track","ids":{"album_id":ALBUM_ID,"track_id":album.tracks[0].id}});
    db.save_track_link(&paths[0], "GB", "[0,0,10,20]", &manual.to_string())
        .await
        .unwrap();
    let summary = link(&db).await;
    assert_eq!(
        (
            summary.total,
            summary.linked,
            summary.review,
            summary.unmatched
        ),
        (3, 2, 1, 0)
    );
    assert_eq!(
        payload(&db, &paths[0]).await,
        manual,
        "An existing manual choice is evidence, not a rewrite target"
    );
    for index in [1, 2] {
        let saved = payload(&db, &paths[index]).await;
        assert_eq!(saved["status"], "linked");
        assert_eq!(saved["ids"]["album_id"], ALBUM_ID);
    }
    let review = payload(&db, &paths[3]).await;
    assert_eq!(review["status"], "review");
    assert!(review["ids"].is_null());
    assert!(review["catalogue_note"]
        .as_str()
        .unwrap()
        .contains("duration differs by 10.00 seconds"));
    let candidate = &review["catalogue_options"][0];
    assert_eq!(candidate["id"], ALBUM_ID);
    assert_eq!(candidate["track_id"], album.tracks[3].id);
    assert_eq!(candidate["compatible"], false);
    assert!(candidate["evidence"]
        .as_str()
        .unwrap()
        .contains("duration differs"));
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn complete_multidisc_total_links_audio_tracks_without_accepting_a_rolling_edition() {
    let (db, directory) = database().await;
    let mut album = release(&[3, 2]);
    album.track_count = 6; // Five audio tracks and one video in the listing.
    let mut rolling = album.clone();
    rolling.id = "810003".into();
    rolling.track_count = 7;
    for (index, track) in rolling.tracks.iter_mut().enumerate() {
        track.id = format!("810003{:02}", index + 1);
    }
    rolling.tracks.push(TidalTrack {
        id: "81000306".into(),
        title: "Bonus recording".into(),
        duration: 201.0,
        isrc: Some("GBTEST260006".into()),
        disc_number: 2,
        track_number: 3,
        ..Default::default()
    });
    cache(&db, vec![rolling, album.clone()]).await;
    let paths = local_files(&db, &album, 5, None, None).await;
    let summary = link(&db).await;
    assert_eq!(
        (
            summary.total,
            summary.linked,
            summary.review,
            summary.unmatched
        ),
        (5, 5, 0, 0)
    );
    for (index, path) in paths.iter().enumerate() {
        let saved = payload(&db, path).await;
        assert_eq!(saved["ids"]["album_id"], ALBUM_ID);
        assert_eq!(saved["ids"]["track_id"], album.tracks[index].id);
        assert!(saved["placements"]
            .as_array()
            .unwrap()
            .iter()
            .all(|placement| placement["album_id"] == ALBUM_ID));
    }
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn two_current_sibling_choices_supply_context_when_local_isrcs_are_missing() {
    let (db, directory) = database().await;
    let album = release(&[4]);
    cache(&db, vec![album.clone()]).await;
    let paths = local_files(&db, &album, 4, Some(3), None).await;
    let connection = db.connect().unwrap();
    for path in &paths[..3] {
        let mut rows = connection
            .query(
                "SELECT metadata FROM local_files WHERE path=?",
                (path.as_str(),),
            )
            .await
            .unwrap();
        let raw: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
        let mut tags: Value = serde_json::from_str(&raw).unwrap();
        tags.as_object_mut().unwrap().remove("isrc");
        drop(rows);
        db.apply_file_update(path, path, ROOT, &tags, 10, 20)
            .await
            .unwrap();
    }
    for (index, path) in paths[..2].iter().enumerate() {
        let choice = json!({"status":"linked","manual":true,"scope":"track","ids":{"album_id":ALBUM_ID,"track_id":album.tracks[index].id}});
        db.save_track_link(path, "GB", "[0,0,10,20]", &choice.to_string())
            .await
            .unwrap();
    }
    let result = link(&db).await;
    assert_eq!((result.total, result.linked, result.review), (2, 2, 0));
    assert_eq!(payload(&db, &paths[3]).await["ids"]["album_id"], ALBUM_ID);
    assert_eq!(
        payload(&db, &paths[3]).await["release_context"]["isrc_conflict"],
        true
    );
    drop(connection);
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}
