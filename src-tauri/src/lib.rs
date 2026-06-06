use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;

struct DbState {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    scan_running: Mutex<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ResourceVariant {
    id: i64,
    path: String,
    file_name: String,
    root_path: String,
    file_size: i64,
    duration_seconds: Option<f64>,
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    resolution: Option<String>,
    source: Option<String>,
    release_group: Option<String>,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    title_guess: String,
    media_kind: String,
    music_artist: Option<String>,
    music_album: Option<String>,
    music_title: Option<String>,
    music_artist_source: Option<String>,
    series_title: Option<String>,
    series_source: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MediaDirectory {
    key: String,
    path: String,
    name: String,
    relative_path: String,
    parent_name: Option<String>,
    file_count: usize,
    total_size: i64,
    files: Vec<ResourceVariant>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MediaGroup {
    key: String,
    name: String,
    subtitle: Option<String>,
    file_count: usize,
    total_size: i64,
    source_keys: Vec<String>,
    files: Vec<ResourceVariant>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LibraryData {
    music_directories: Vec<MediaDirectory>,
    video_directories: Vec<MediaDirectory>,
    music_artists: Vec<MediaGroup>,
    video_series: Vec<MediaGroup>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanSummary {
    scanned_files: usize,
    imported_files: usize,
    skipped_files: usize,
    skipped_short_files: usize,
    recorded_directories: usize,
    ffprobe_missing: bool,
    library: LibraryData,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    phase: String,
    discovered_files: usize,
    processed_files: usize,
    imported_files: usize,
    skipped_files: usize,
    skipped_short_files: usize,
    total_files: Option<usize>,
    current_path: Option<String>,
    ffprobe_missing: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeRequest {
    kind: String,
    source_keys: Vec<String>,
    target_name: String,
}

#[derive(Debug, Default)]
struct MediaProbe {
    duration_seconds: Option<f64>,
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    has_video: bool,
    has_audio: bool,
    music_artist: Option<String>,
    music_album: Option<String>,
    music_title: Option<String>,
}

#[derive(Debug)]
struct ParsedName {
    title_guess: String,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    source: Option<String>,
    release_group: Option<String>,
}

#[derive(Debug)]
struct AnalyzedFile {
    path: String,
    file_name: String,
    root_path: String,
    file_size: i64,
    modified_ms: Option<i64>,
    duration_seconds: Option<f64>,
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    resolution: Option<String>,
    source: Option<String>,
    release_group: Option<String>,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    title_guess: String,
    item_key: String,
    music_artist: Option<String>,
    music_album: Option<String>,
    music_title: Option<String>,
    music_artist_source: Option<String>,
    series_title: Option<String>,
    series_source: Option<String>,
}

#[derive(Debug)]
enum AnalyzeSkip {
    Other,
    ShortVideo,
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Option<Vec<FfprobeStream>>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    duration: Option<String>,
    disposition: Option<FfprobeDisposition>,
    tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct FfprobeDisposition {
    attached_pic: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    format_name: Option<String>,
    duration: Option<String>,
    tags: Option<HashMap<String, String>>,
}

const MEDIA_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "m4v", "ts", "m2ts", "webm", "mpg", "mpeg", "mp3",
    "flac", "m4a", "aac", "ogg", "opus", "wav", "ape",
];
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "ape"];
const MIN_VIDEO_SECONDS: f64 = 300.0;

#[tauri::command]
fn check_ffprobe() -> Result<String, String> {
    match Command::new("ffprobe").arg("-version").output() {
        Ok(output) if output.status.success() => Ok("ffprobe 已从 PATH 找到".to_string()),
        Ok(_) => Err("ffprobe 可执行文件存在，但版本检查失败".to_string()),
        Err(error) => Err(format!("未能从 PATH 调用 ffprobe：{error}")),
    }
}

#[tauri::command]
fn start_scan(paths: Vec<String>, app: AppHandle, db: State<'_, DbState>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("至少需要选择一个目录".to_string());
    }

    {
        let mut running = db
            .scan_running
            .lock()
            .map_err(|_| "扫描状态被占用，稍后再试".to_string())?;
        if *running {
            return Err("已有扫描任务正在运行".to_string());
        }
        *running = true;
    }

    let db_path = db.db_path.clone();
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_scan(paths, &db_path, Some(&app_handle));
        match result {
            Ok(summary) => {
                let _ = app_handle.emit("scan-complete", summary);
            }
            Err(error) => {
                let _ = app_handle.emit("scan-error", error);
            }
        }

        {
            let state = app_handle.state::<DbState>();
            if let Ok(mut running) = state.scan_running.lock() {
                *running = false;
            };
        }
    });

    Ok(())
}

fn run_scan(
    paths: Vec<String>,
    db_path: &Path,
    app: Option<&AppHandle>,
) -> Result<ScanSummary, String> {
    let ffprobe_missing = Command::new("ffprobe").arg("-version").output().is_err();
    let roots_for_cleanup = paths.clone();
    let mut discovered_files = 0;
    let mut processed_files = 0;
    let mut imported_files = 0;
    let mut skipped_files = 0;
    let mut skipped_short_files = 0;
    let mut media_files: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut last_emit = Instant::now();

    maybe_emit_scan_progress(
        app,
        "discovering",
        discovered_files,
        processed_files,
        imported_files,
        skipped_files,
        skipped_short_files,
        None,
        None,
        ffprobe_missing,
        &mut last_emit,
        true,
    );

    for root in paths {
        let root_path = PathBuf::from(&root);
        if !root_path.exists() {
            skipped_files += 1;
            maybe_emit_scan_progress(
                app,
                "discovering",
                discovered_files,
                processed_files,
                imported_files,
                skipped_files,
                skipped_short_files,
                None,
                Some(root),
                ffprobe_missing,
                &mut last_emit,
                true,
            );
            continue;
        }

        for entry in WalkDir::new(&root_path).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    skipped_files += 1;
                    continue;
                }
            };

            let path = entry.path();
            if !entry.file_type().is_file() || !is_media_file(path) {
                continue;
            }

            discovered_files += 1;
            media_files.push((path.to_path_buf(), root_path.clone()));
            maybe_emit_scan_progress(
                app,
                "discovering",
                discovered_files,
                processed_files,
                imported_files,
                skipped_files,
                skipped_short_files,
                None,
                Some(path.to_string_lossy().to_string()),
                ffprobe_missing,
                &mut last_emit,
                false,
            );
        }
    }

    let total_files = media_files.len();
    let conn = Connection::open(db_path).map_err(|error| error.to_string())?;
    init_schema(&conn).map_err(|error| error.to_string())?;
    delete_roots(&conn, &roots_for_cleanup)?;
    maybe_emit_scan_progress(
        app,
        "processing",
        discovered_files,
        processed_files,
        imported_files,
        skipped_files,
        skipped_short_files,
        Some(total_files),
        None,
        ffprobe_missing,
        &mut last_emit,
        true,
    );

    for (path, root_path) in media_files {
        processed_files += 1;
        match analyze_file(&path, &root_path, !ffprobe_missing) {
            Ok(file) => {
                upsert_file(&conn, &file)?;
                imported_files += 1;
            }
            Err(AnalyzeSkip::ShortVideo) => {
                skipped_files += 1;
                skipped_short_files += 1;
            }
            Err(AnalyzeSkip::Other) => {
                skipped_files += 1;
            }
        }

        maybe_emit_scan_progress(
            app,
            "processing",
            discovered_files,
            processed_files,
            imported_files,
            skipped_files,
            skipped_short_files,
            Some(total_files),
            Some(path.to_string_lossy().to_string()),
            ffprobe_missing,
            &mut last_emit,
            false,
        );
    }

    maybe_emit_scan_progress(
        app,
        "processing",
        discovered_files,
        processed_files,
        imported_files,
        skipped_files,
        skipped_short_files,
        Some(total_files),
        None,
        ffprobe_missing,
        &mut last_emit,
        true,
    );

    let library = load_library(&conn)?;
    let recorded_directories = library.music_directories.len() + library.video_directories.len();

    Ok(ScanSummary {
        scanned_files: processed_files,
        imported_files,
        skipped_files,
        skipped_short_files,
        recorded_directories,
        ffprobe_missing,
        library,
    })
}

#[allow(clippy::too_many_arguments)]
fn maybe_emit_scan_progress(
    app: Option<&AppHandle>,
    phase: &str,
    discovered_files: usize,
    processed_files: usize,
    imported_files: usize,
    skipped_files: usize,
    skipped_short_files: usize,
    total_files: Option<usize>,
    current_path: Option<String>,
    ffprobe_missing: bool,
    last_emit: &mut Instant,
    force: bool,
) {
    if !force && last_emit.elapsed() < Duration::from_millis(250) {
        return;
    }

    if let Some(app) = app {
        let progress = ScanProgress {
            phase: phase.to_string(),
            discovered_files,
            processed_files,
            imported_files,
            skipped_files,
            skipped_short_files,
            total_files,
            current_path,
            ffprobe_missing,
        };
        let _ = app.emit("scan-progress", progress);
    }

    *last_emit = Instant::now();
}

#[tauri::command]
fn list_library(db: State<'_, DbState>) -> Result<LibraryData, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "数据库连接被占用，稍后再试".to_string())?;
    load_library(&conn)
}

#[tauri::command]
fn set_merge_rules(request: MergeRequest, db: State<'_, DbState>) -> Result<LibraryData, String> {
    if !matches!(request.kind.as_str(), "music_artist" | "video_series") {
        return Err("不支持的合并类型".to_string());
    }

    let mut source_keys: Vec<String> = Vec::new();
    for key in request.source_keys {
        let trimmed = key.trim();
        if !trimmed.is_empty() && !source_keys.iter().any(|existing| existing == trimmed) {
            source_keys.push(trimmed.to_string());
        }
    }
    if source_keys.is_empty() {
        return Err("没有可合并的来源".to_string());
    }

    let target_name = request.target_name.trim();
    let conn = db
        .conn
        .lock()
        .map_err(|_| "数据库连接被占用，稍后再试".to_string())?;

    if target_name.is_empty() {
        for source_key in source_keys {
            conn.execute(
                "DELETE FROM merge_rules WHERE kind = ?1 AND source_key = ?2",
                params![&request.kind, source_key],
            )
            .map_err(|error| error.to_string())?;
        }
    } else {
        for source_key in source_keys {
            conn.execute(
                r#"
                INSERT INTO merge_rules (kind, source_key, target_name)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(kind, source_key) DO UPDATE SET
                  target_name = excluded.target_name,
                  updated_at = CURRENT_TIMESTAMP
                "#,
                params![&request.kind, source_key, target_name],
            )
            .map_err(|error| error.to_string())?;
        }
    }

    load_library(&conn)
}

fn analyze_file(
    path: &Path,
    root_path: &Path,
    use_ffprobe: bool,
) -> Result<AnalyzedFile, AnalyzeSkip> {
    let metadata = fs::metadata(path).map_err(|_| AnalyzeSkip::Other)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let parsed = parse_name(&file_name);
    let probe = if use_ffprobe {
        probe_media(path).map_err(|_| AnalyzeSkip::Other)?
    } else {
        return Err(AnalyzeSkip::Other);
    };

    if !probe.has_video && !probe.has_audio {
        return Err(AnalyzeSkip::Other);
    }

    if is_short_video(&probe) {
        return Err(AnalyzeSkip::ShortVideo);
    }

    let resolution = resolution_from_probe_or_name(probe.width, probe.height, &file_name);
    let (music_artist, music_album, music_title, music_artist_source) =
        if probe.has_audio && !probe.has_video {
            infer_music_metadata(&probe, path, root_path, &file_name)
        } else {
            (None, None, None, None)
        };
    let (series_title, series_source) = if probe.has_video {
        infer_video_series(path, root_path, &parsed)
    } else {
        (None, None)
    };
    let title_guess = if probe.has_video {
        series_title
            .clone()
            .unwrap_or_else(|| parsed.title_guess.clone())
    } else {
        music_title
            .clone()
            .unwrap_or_else(|| parsed.title_guess.clone())
    };
    let item_key = build_item_key(&title_guess, parsed.season_number, parsed.episode_number);
    let path_string = path.to_string_lossy().to_string();
    let root_string = root_path.to_string_lossy().to_string();
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);

    Ok(AnalyzedFile {
        path: path_string,
        file_name,
        root_path: root_string,
        file_size: metadata.len() as i64,
        modified_ms,
        duration_seconds: probe.duration_seconds,
        container: probe.container,
        video_codec: probe.video_codec,
        audio_codec: probe.audio_codec,
        width: probe.width,
        height: probe.height,
        resolution,
        source: parsed.source,
        release_group: parsed.release_group,
        season_number: parsed.season_number,
        episode_number: parsed.episode_number,
        title_guess,
        item_key,
        music_artist,
        music_album,
        music_title,
        music_artist_source,
        series_title,
        series_source,
    })
}

fn is_short_video(probe: &MediaProbe) -> bool {
    probe.has_video
        && probe
            .duration_seconds
            .map(|duration| duration < MIN_VIDEO_SECONDS)
            .unwrap_or(false)
}

fn probe_media(path: &Path) -> Result<MediaProbe, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|error| error.to_string())?;

    if !output.status.success() {
        return Err("ffprobe 分析失败".to_string());
    }

    let parsed: FfprobeOutput =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let mut probe = MediaProbe::default();

    if let Some(format) = parsed.format {
        let tags = format.tags;
        probe.container = format.format_name;
        probe.duration_seconds = format.duration.and_then(|value| value.parse::<f64>().ok());
        if let Some(tags) = tags {
            probe.music_artist = tag_value(
                &tags,
                &["album_artist", "albumartist", "artist", "composer"],
            );
            probe.music_album = tag_value(&tags, &["album"]);
            probe.music_title = tag_value(&tags, &["title"]);
        }
    }

    if let Some(streams) = parsed.streams {
        for stream in streams {
            match stream.codec_type.as_deref() {
                Some("video") => {
                    // FLAC/MP3 album art is reported by ffprobe as an attached video stream.
                    if stream
                        .disposition
                        .as_ref()
                        .and_then(|disposition| disposition.attached_pic)
                        .unwrap_or(0)
                        == 1
                    {
                        continue;
                    }
                    probe.has_video = true;
                    if probe.video_codec.is_none() {
                        probe.video_codec = stream
                            .codec_name
                            .map(normalize_video_codec)
                            .or_else(|| Some("Unknown Video".to_string()));
                        probe.width = stream.width;
                        probe.height = stream.height;
                        if probe.duration_seconds.is_none() {
                            probe.duration_seconds =
                                stream.duration.and_then(|value| value.parse::<f64>().ok());
                        }
                    }
                }
                Some("audio") => {
                    probe.has_audio = true;
                    if let Some(tags) = stream.tags.as_ref() {
                        if probe.music_artist.is_none() {
                            probe.music_artist = tag_value(
                                tags,
                                &["album_artist", "albumartist", "artist", "composer"],
                            );
                        }
                        if probe.music_album.is_none() {
                            probe.music_album = tag_value(tags, &["album"]);
                        }
                        if probe.music_title.is_none() {
                            probe.music_title = tag_value(tags, &["title"]);
                        }
                    }
                    if probe.audio_codec.is_none() {
                        probe.audio_codec = stream
                            .codec_name
                            .map(normalize_audio_codec)
                            .or_else(|| Some("Unknown Audio".to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(probe)
}

fn tag_value(tags: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    for wanted in keys {
        for (key, value) in tags {
            if key.eq_ignore_ascii_case(wanted) {
                let cleaned = clean_metadata_value(value);
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }
    None
}

fn clean_metadata_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn infer_music_metadata(
    probe: &MediaProbe,
    path: &Path,
    root_path: &Path,
    file_name: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let album = probe
        .music_album
        .clone()
        .or_else(|| infer_album_from_directory(path, root_path));
    let filename_artist_title = infer_artist_title_from_filename(file_name);
    let title = probe
        .music_title
        .clone()
        .or_else(|| {
            filename_artist_title
                .as_ref()
                .map(|(_, title)| title.clone())
        })
        .or_else(|| infer_music_title_from_filename(file_name));

    if let Some(artist) = probe.music_artist.clone().filter(|value| !value.is_empty()) {
        return (Some(artist), album, title, Some("tag".to_string()));
    }

    if let Some(artist) = infer_artist_from_directory(path, root_path) {
        return (Some(artist), album, title, Some("directory".to_string()));
    }

    if let Some((artist, _)) = filename_artist_title {
        return (Some(artist), album, title, Some("filename".to_string()));
    }

    (
        Some("未知作者".to_string()),
        album,
        title,
        Some("unknown".to_string()),
    )
}

fn infer_artist_from_directory(path: &Path, root_path: &Path) -> Option<String> {
    let components = relative_parent_components(path, root_path);
    let candidate = match components.len() {
        0 => None,
        1 => components.last(),
        _ => components.get(components.len() - 2),
    }?;
    clean_group_name(candidate).filter(|value| !is_generic_music_directory(value))
}

fn infer_album_from_directory(path: &Path, root_path: &Path) -> Option<String> {
    relative_parent_components(path, root_path)
        .last()
        .and_then(|value| clean_group_name(value))
}

fn infer_music_title_from_filename(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name);
    let title = clean_title(stem);
    (title != "未命名资源").then_some(title)
}

fn infer_artist_title_from_filename(file_name: &str) -> Option<(String, String)> {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name);
    let pattern = Regex::new(r"^\s*(.+?)\s+-\s+(.+?)\s*$").ok()?;
    let caps = pattern.captures(stem)?;
    let artist = clean_group_name(caps.get(1)?.as_str())?;
    let title = clean_group_name(caps.get(2)?.as_str())?;
    (!artist.is_empty() && !title.is_empty()).then_some((artist, title))
}

fn infer_video_series(
    path: &Path,
    root_path: &Path,
    parsed: &ParsedName,
) -> (Option<String>, Option<String>) {
    if is_detected_title(&parsed.title_guess) {
        return (
            Some(parsed.title_guess.clone()),
            Some("filename".to_string()),
        );
    }

    if let Some(series) = infer_series_from_directory(path, root_path) {
        return (Some(series), Some("directory".to_string()));
    }

    if parsed.title_guess != "未命名资源" {
        return (
            Some(parsed.title_guess.clone()),
            Some("filename".to_string()),
        );
    }

    (Some("未识别系列".to_string()), Some("unknown".to_string()))
}

fn infer_series_from_directory(path: &Path, root_path: &Path) -> Option<String> {
    let components = relative_parent_components(path, root_path);
    for component in components.iter().rev() {
        if let Some(candidate) = clean_group_name(component) {
            if !is_generic_video_directory(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn relative_parent_components(path: &Path, root_path: &Path) -> Vec<String> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let relative = parent.strip_prefix(root_path).unwrap_or(parent);
    relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter_map(clean_group_name)
        .collect()
}

fn clean_group_name(value: &str) -> Option<String> {
    let cleaned = value
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(&['-', '.', '_', ' '][..])
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn is_detected_title(value: &str) -> bool {
    let key = normalize_key(value);
    value != "未命名资源"
        && key != "unknown"
        && !key.is_empty()
        && !key.chars().all(|ch| ch.is_ascii_digit())
}

fn is_generic_music_directory(value: &str) -> bool {
    matches!(
        normalize_key(value).as_str(),
        "ost" | "music" | "audio" | "songs" | "soundtrack" | "soundtracks" | "cd" | "disc"
    )
}

fn is_generic_video_directory(value: &str) -> bool {
    let key = normalize_key(value);
    if Regex::new(r"^s\d{1,2}$")
        .expect("valid season directory regex")
        .is_match(&key)
    {
        return true;
    }

    matches!(
        key.as_str(),
        "season"
            | "season-1"
            | "season-01"
            | "extras"
            | "extra"
            | "specials"
            | "ova"
            | "oad"
            | "sp"
            | "subs"
            | "subtitle"
            | "subtitles"
            | "bdmv"
            | "stream"
            | "video"
            | "videos"
            | "movie"
            | "movies"
    )
}

fn build_item_key(title: &str, season_number: Option<i64>, episode_number: Option<i64>) -> String {
    let normalized_title = normalize_key(title);
    match (season_number, episode_number) {
        (Some(season), Some(episode)) => {
            format!("episode:{normalized_title}:s{season:02}:e{episode:03}")
        }
        _ => format!("item:{normalized_title}"),
    }
}

fn parse_bracketed_anime_name(
    stem: &str,
    fallback_release_group: Option<String>,
    source: Option<String>,
) -> Option<ParsedName> {
    let pattern =
        Regex::new(r"^\[([^\]]{2,40})\]\s*\[([^\]]{1,160})\]\s*\[(\d{1,4})(?:v\d+)?\]").ok()?;
    let caps = pattern.captures(stem)?;
    let release_group = caps
        .get(1)
        .map(|value| value.as_str().trim().to_string())
        .filter(|value| !value.is_empty())
        .or(fallback_release_group);
    let title_guess = clean_group_name(caps.get(2)?.as_str())?;
    let episode_number = caps.get(3)?.as_str().parse::<i64>().ok()?;
    let season_number = Some(1);
    let episode_number = Some(episode_number);
    Some(ParsedName {
        title_guess,
        season_number,
        episode_number,
        source,
        release_group,
    })
}

fn parse_name(file_name: &str) -> ParsedName {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name);
    let release_group = detect_release_group(stem);
    let source = detect_source(stem);
    if let Some(parsed) = parse_bracketed_anime_name(stem, release_group.clone(), source.clone()) {
        return parsed;
    }
    let without_group = strip_release_group_prefix(stem);
    let season_episode = detect_season_episode(&without_group);
    let anime_episode = if season_episode.is_none() {
        detect_anime_episode(&without_group)
    } else {
        None
    };

    let (title_source, season_number, episode_number) =
        if let Some((start, season, episode)) = season_episode {
            (&without_group[..start], Some(season), Some(episode))
        } else if let Some((start, episode)) = anime_episode {
            (&without_group[..start], Some(1), Some(episode))
        } else {
            (without_group.as_str(), None, None)
        };

    let title_guess = clean_title(title_source);

    ParsedName {
        title_guess,
        season_number,
        episode_number,
        source,
        release_group,
    }
}

fn detect_season_episode(value: &str) -> Option<(usize, i64, i64)> {
    let patterns = [
        Regex::new(r"(?i)S(\d{1,2})[\s._-]*E(\d{1,3})").ok()?,
        Regex::new(r"(?i)(\d{1,2})x(\d{1,3})").ok()?,
    ];

    for pattern in patterns {
        if let Some(caps) = pattern.captures(value) {
            let whole = caps.get(0)?;
            let season = caps.get(1)?.as_str().parse::<i64>().ok()?;
            let episode = caps.get(2)?.as_str().parse::<i64>().ok()?;
            return Some((whole.start(), season, episode));
        }
    }

    None
}

fn detect_anime_episode(value: &str) -> Option<(usize, i64)> {
    let patterns = [
        Regex::new(r"(?i)\s-\s(\d{1,3})(?:v\d+)?(?:\s|$|\[|\()").ok()?,
        Regex::new(r"(?i)[\s._-](\d{1,3})(?:v\d+)?(?:\s|$|\[|\()").ok()?,
    ];

    for pattern in patterns {
        if let Some(caps) = pattern.captures(value) {
            let whole = caps.get(0)?;
            let episode = caps.get(1)?.as_str().parse::<i64>().ok()?;
            if (1..=999).contains(&episode) {
                return Some((whole.start(), episode));
            }
        }
    }

    None
}

fn detect_release_group(stem: &str) -> Option<String> {
    let prefix = Regex::new(r"^\[([^\]]{2,40})\]").ok()?;
    if let Some(caps) = prefix.captures(stem) {
        return caps.get(1).map(|value| value.as_str().trim().to_string());
    }

    let suffix = Regex::new(r"-([A-Za-z0-9][A-Za-z0-9._-]{1,39})$").ok()?;
    suffix
        .captures(stem)
        .and_then(|caps| caps.get(1))
        .map(|value| {
            value
                .as_str()
                .trim_matches(&['.', '_', '-'][..])
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn strip_release_group_prefix(stem: &str) -> String {
    let prefix = Regex::new(r"^\[[^\]]{2,40}\]\s*").expect("valid release group regex");
    prefix.replace(stem, "").into_owned()
}

fn detect_source(value: &str) -> Option<String> {
    let normalized = value.replace('.', " ").replace('_', " ").to_uppercase();
    let candidates = [
        ("UHD BLURAY", "UHD BluRay"),
        ("BDREMUX", "BD Remux"),
        ("REMUX", "Remux"),
        ("BLU RAY", "BluRay"),
        ("BLURAY", "BluRay"),
        ("BDRIP", "BDRip"),
        ("WEB DL", "WEB-DL"),
        ("WEBDL", "WEB-DL"),
        ("WEBRIP", "WEBRip"),
        ("HDTV", "HDTV"),
        ("DVDRIP", "DVDRip"),
        ("DVD", "DVD"),
    ];

    candidates
        .iter()
        .find(|(needle, _)| normalized.contains(needle))
        .map(|(_, label)| (*label).to_string())
}

fn clean_title(value: &str) -> String {
    let tag_pattern = Regex::new(
        r"(?i)\b(2160p|1080p|720p|480p|4k|8k|web[- ]?dl|webrip|bdrip|blu[- ]?ray|bdremux|remux|hdtv|x264|x265|h\.?264|h\.?265|hevc|av1|aac|flac|truehd|dts|10bit|8bit|hdr|dv)\b",
    )
    .expect("valid tag regex");
    let bracket_pattern = Regex::new(r"[\[\(][^\]\)]*[\]\)]").expect("valid bracket regex");
    let separated = value.replace('.', " ").replace('_', " ");
    let no_tags = tag_pattern.replace_all(&separated, " ");
    let no_brackets = bracket_pattern.replace_all(&no_tags, " ");
    let cleaned = no_brackets
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(&['-', '.', '_', ' '][..])
        .to_string();

    if cleaned.is_empty() {
        "未命名资源".to_string()
    } else {
        cleaned
    }
}

fn normalize_key(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn resolution_from_probe_or_name(
    width: Option<i64>,
    height: Option<i64>,
    file_name: &str,
) -> Option<String> {
    if let Some(height) = height {
        let label = if height >= 2000 {
            "2160p".to_string()
        } else if height >= 1000 {
            "1080p".to_string()
        } else if height >= 700 {
            "720p".to_string()
        } else if height >= 470 {
            "480p".to_string()
        } else {
            format!("{height}p")
        };
        return Some(match width {
            Some(width) => format!("{label} ({width}x{height})"),
            None => label,
        });
    }

    let pattern = Regex::new(r"(?i)\b(2160p|1080p|720p|480p|4k|8k)\b").ok()?;
    pattern
        .captures(file_name)
        .and_then(|caps| caps.get(1))
        .map(|value| value.as_str().to_string())
}

fn normalize_video_codec(codec: String) -> String {
    match codec.as_str() {
        "h264" => "H.264".to_string(),
        "hevc" => "H.265 / HEVC".to_string(),
        "av1" => "AV1".to_string(),
        "vp9" => "VP9".to_string(),
        other => other.to_string(),
    }
}

fn normalize_audio_codec(codec: String) -> String {
    match codec.as_str() {
        "aac" => "AAC".to_string(),
        "flac" => "FLAC".to_string(),
        "mp3" => "MP3".to_string(),
        "opus" => "Opus".to_string(),
        "vorbis" => "Vorbis".to_string(),
        "truehd" => "TrueHD".to_string(),
        "dts" => "DTS".to_string(),
        other => other.to_string(),
    }
}

fn is_media_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|ext| MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn is_audio_file_name(file_name: &str) -> bool {
    Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn media_kind_for_file(file: &ResourceVariant) -> Option<String> {
    if is_audio_file_name(&file.file_name) {
        return file.audio_codec.as_ref().map(|_| "music".to_string());
    }

    if file.video_codec.is_some() || file.width.is_some() || file.height.is_some() {
        Some("video".to_string())
    } else if file.audio_codec.is_some() {
        Some("music".to_string())
    } else {
        None
    }
}

fn upsert_file(conn: &Connection, file: &AnalyzedFile) -> Result<(), String> {
    conn.execute(
        r#"
      INSERT INTO media_files (
        path, file_name, root_path, file_size, modified_ms, duration_seconds, container,
        video_codec, audio_codec, width, height, resolution, source, release_group,
        season_number, episode_number, title_guess, item_key, music_artist, music_album,
        music_title, music_artist_source, series_title, series_source
      )
      VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
        ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
      )
      ON CONFLICT(path) DO UPDATE SET
        file_name = excluded.file_name,
        root_path = excluded.root_path,
        file_size = excluded.file_size,
        modified_ms = excluded.modified_ms,
        duration_seconds = excluded.duration_seconds,
        container = excluded.container,
        video_codec = excluded.video_codec,
        audio_codec = excluded.audio_codec,
        width = excluded.width,
        height = excluded.height,
        resolution = excluded.resolution,
        source = excluded.source,
        release_group = excluded.release_group,
        season_number = excluded.season_number,
        episode_number = excluded.episode_number,
        title_guess = excluded.title_guess,
        item_key = excluded.item_key,
        music_artist = excluded.music_artist,
        music_album = excluded.music_album,
        music_title = excluded.music_title,
        music_artist_source = excluded.music_artist_source,
        series_title = excluded.series_title,
        series_source = excluded.series_source,
        updated_at = CURRENT_TIMESTAMP
      "#,
        params![
            &file.path,
            &file.file_name,
            &file.root_path,
            file.file_size,
            file.modified_ms,
            file.duration_seconds,
            file.container.as_deref(),
            file.video_codec.as_deref(),
            file.audio_codec.as_deref(),
            file.width,
            file.height,
            file.resolution.as_deref(),
            file.source.as_deref(),
            file.release_group.as_deref(),
            file.season_number,
            file.episode_number,
            &file.title_guess,
            &file.item_key,
            file.music_artist.as_deref(),
            file.music_album.as_deref(),
            file.music_title.as_deref(),
            file.music_artist_source.as_deref(),
            file.series_title.as_deref(),
            file.series_source.as_deref()
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn delete_roots(conn: &Connection, roots: &[String]) -> Result<(), String> {
    for root in roots {
        conn.execute(
            "DELETE FROM media_files WHERE root_path = ?1",
            params![root],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn load_library(conn: &Connection) -> Result<LibraryData, String> {
    let music_artist_rules = load_merge_rules(conn, "music_artist")?;
    let video_series_rules = load_merge_rules(conn, "video_series")?;
    let mut stmt = conn
        .prepare(
            r#"
      SELECT
        id, path, file_name, root_path, file_size, duration_seconds, container,
        video_codec, audio_codec, width, height, resolution, source, release_group,
        season_number, episode_number, title_guess, item_key, music_artist, music_album,
        music_title, music_artist_source, series_title, series_source
      FROM media_files
      ORDER BY root_path COLLATE NOCASE, path COLLATE NOCASE
      "#,
        )
        .map_err(|error| error.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let mut file = ResourceVariant {
                id: row.get(0)?,
                path: row.get(1)?,
                file_name: row.get(2)?,
                root_path: row.get(3)?,
                file_size: row.get(4)?,
                duration_seconds: row.get(5)?,
                container: row.get(6)?,
                video_codec: row.get(7)?,
                audio_codec: row.get(8)?,
                width: row.get(9)?,
                height: row.get(10)?,
                resolution: row.get(11)?,
                source: row.get(12)?,
                release_group: row.get(13)?,
                season_number: row.get(14)?,
                episode_number: row.get(15)?,
                title_guess: row.get(16)?,
                media_kind: String::new(),
                music_artist: row.get(18)?,
                music_album: row.get(19)?,
                music_title: row.get(20)?,
                music_artist_source: row.get(21)?,
                series_title: row.get(22)?,
                series_source: row.get(23)?,
            };
            file.media_kind = media_kind_for_file(&file).unwrap_or_else(|| "ignored".to_string());
            Ok(file)
        })
        .map_err(|error| error.to_string())?;

    let mut music_directories: Vec<MediaDirectory> = Vec::new();
    let mut video_directories: Vec<MediaDirectory> = Vec::new();
    let mut music_artists: Vec<MediaGroup> = Vec::new();
    let mut video_series: Vec<MediaGroup> = Vec::new();

    for row in rows {
        let file = row.map_err(|error| error.to_string())?;
        match file.media_kind.as_str() {
            "music" => {
                push_file_into_directory(&mut music_directories, file.clone());
                push_music_file_into_artist_group(&mut music_artists, file, &music_artist_rules);
            }
            "video" => {
                push_file_into_directory(&mut video_directories, file.clone());
                push_video_file_into_series_group(&mut video_series, file, &video_series_rules);
            }
            _ => {}
        }
    }

    sort_directories(&mut music_directories);
    sort_directories(&mut video_directories);
    sort_groups(&mut music_artists);
    sort_groups(&mut video_series);

    Ok(LibraryData {
        music_directories,
        video_directories,
        music_artists,
        video_series,
    })
}

fn load_merge_rules(conn: &Connection, kind: &str) -> Result<HashMap<String, String>, String> {
    let mut stmt = conn
        .prepare("SELECT source_key, target_name FROM merge_rules WHERE kind = ?1")
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?;

    let mut rules = HashMap::new();
    for row in rows {
        let (source_key, target_name) = row.map_err(|error| error.to_string())?;
        rules.insert(source_key, target_name);
    }
    Ok(rules)
}

fn push_music_file_into_artist_group(
    groups: &mut Vec<MediaGroup>,
    file: ResourceVariant,
    rules: &HashMap<String, String>,
) {
    let artist = file
        .music_artist
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| infer_artist_from_directory(Path::new(&file.path), Path::new(&file.root_path)))
        .unwrap_or_else(|| "未知作者".to_string());
    let source_key = group_source_key("music_artist", &artist);
    let subtitle = file
        .music_artist_source
        .as_deref()
        .map(music_artist_source_label);
    push_file_into_group(
        groups,
        "music_artist",
        source_key,
        artist,
        subtitle,
        file,
        rules,
    );
}

fn push_video_file_into_series_group(
    groups: &mut Vec<MediaGroup>,
    file: ResourceVariant,
    rules: &HashMap<String, String>,
) {
    let series = file
        .series_title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            if is_detected_title(&file.title_guess) {
                Some(file.title_guess.clone())
            } else {
                infer_series_from_directory(Path::new(&file.path), Path::new(&file.root_path))
            }
        })
        .unwrap_or_else(|| "未识别系列".to_string());
    let source_key = group_source_key("video_series", &series);
    let subtitle = file.series_source.as_deref().map(series_source_label);
    push_file_into_group(
        groups,
        "video_series",
        source_key,
        series,
        subtitle,
        file,
        rules,
    );
}

fn push_file_into_group(
    groups: &mut Vec<MediaGroup>,
    kind: &str,
    source_key: String,
    detected_name: String,
    subtitle: Option<String>,
    file: ResourceVariant,
    rules: &HashMap<String, String>,
) {
    let display_name = rules
        .get(&source_key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or(detected_name);
    let group_key = group_source_key(kind, &display_name);

    let group_index = match groups.iter().position(|group| group.key == group_key) {
        Some(index) => index,
        None => {
            groups.push(MediaGroup {
                key: group_key.clone(),
                name: display_name,
                subtitle: subtitle.clone(),
                file_count: 0,
                total_size: 0,
                source_keys: Vec::new(),
                files: Vec::new(),
            });
            groups.len() - 1
        }
    };

    let group = &mut groups[group_index];
    if !group.source_keys.iter().any(|key| key == &source_key) {
        group.source_keys.push(source_key);
    }
    if group.subtitle != subtitle {
        group.subtitle = None;
    }
    group.file_count += 1;
    group.total_size += file.file_size;
    group.files.push(file);
}

fn group_source_key(kind: &str, name: &str) -> String {
    let normalized = normalize_key(name);
    if normalized.is_empty() {
        format!("{kind}:unknown")
    } else {
        format!("{kind}:{normalized}")
    }
}

fn music_artist_source_label(source: &str) -> String {
    match source {
        "tag" => "标签".to_string(),
        "directory" => "目录".to_string(),
        "filename" => "文件名".to_string(),
        _ => "未知来源".to_string(),
    }
}

fn series_source_label(source: &str) -> String {
    match source {
        "filename" => "文件名".to_string(),
        "directory" => "目录".to_string(),
        _ => "未知来源".to_string(),
    }
}

fn push_file_into_directory(directories: &mut Vec<MediaDirectory>, file: ResourceVariant) {
    let dir_path = Path::new(&file.path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| file.root_path.clone());

    let dir_name = Path::new(&dir_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("根目录")
        .to_string();
    let parent_name = Path::new(&dir_path)
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
        .map(|value| value.to_string());
    let relative_path = relative_directory_path(&file.root_path, &dir_path);

    let directory_index = match directories
        .iter()
        .position(|directory| directory.key == dir_path)
    {
        Some(index) => index,
        None => {
            directories.push(MediaDirectory {
                key: dir_path.clone(),
                path: dir_path.clone(),
                name: dir_name,
                relative_path,
                parent_name,
                file_count: 0,
                total_size: 0,
                files: Vec::new(),
            });
            directories.len() - 1
        }
    };

    let directory = &mut directories[directory_index];
    directory.file_count += 1;
    directory.total_size += file.file_size;
    directory.files.push(file);
}

fn relative_directory_path(root_path: &str, dir_path: &str) -> String {
    let root = Path::new(root_path);
    let dir = Path::new(dir_path);
    if let Ok(relative) = dir.strip_prefix(root) {
        let value = relative.to_string_lossy().replace('\\', "/");
        if !value.trim().is_empty() {
            return value;
        }
    }

    Path::new(dir_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("根目录")
        .to_string()
}

fn sort_directories(directories: &mut [MediaDirectory]) {
    directories.sort_by(|a, b| {
        a.relative_path
            .to_lowercase()
            .cmp(&b.relative_path.to_lowercase())
    });
    for directory in directories {
        directory.files.sort_by(|a, b| {
            let season_cmp = a.season_number.cmp(&b.season_number);
            if season_cmp != std::cmp::Ordering::Equal {
                return season_cmp;
            }
            let episode_cmp = a.episode_number.cmp(&b.episode_number);
            if episode_cmp != std::cmp::Ordering::Equal {
                return episode_cmp;
            }
            a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase())
        });
    }
}

fn sort_groups(groups: &mut [MediaGroup]) {
    groups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    for group in groups {
        group.source_keys.sort();
        group.files.sort_by(|a, b| {
            let season_cmp = a.season_number.cmp(&b.season_number);
            if season_cmp != std::cmp::Ordering::Equal {
                return season_cmp;
            }
            let episode_cmp = a.episode_number.cmp(&b.episode_number);
            if episode_cmp != std::cmp::Ordering::Equal {
                return episode_cmp;
            }
            a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase())
        });
    }
}

fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        r#"
    CREATE TABLE IF NOT EXISTS media_files (
      id INTEGER PRIMARY KEY,
      path TEXT NOT NULL UNIQUE,
      file_name TEXT NOT NULL,
      root_path TEXT NOT NULL,
      file_size INTEGER NOT NULL,
      modified_ms INTEGER,
      duration_seconds REAL,
      container TEXT,
      video_codec TEXT,
      audio_codec TEXT,
      width INTEGER,
      height INTEGER,
      resolution TEXT,
      source TEXT,
      release_group TEXT,
      season_number INTEGER,
      episode_number INTEGER,
      title_guess TEXT NOT NULL,
      item_key TEXT NOT NULL,
      music_artist TEXT,
      music_album TEXT,
      music_title TEXT,
      music_artist_source TEXT,
      series_title TEXT,
      series_source TEXT,
      created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
      updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

    CREATE TABLE IF NOT EXISTS merge_rules (
      kind TEXT NOT NULL,
      source_key TEXT NOT NULL,
      target_name TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
      updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
      PRIMARY KEY (kind, source_key)
    );

    CREATE INDEX IF NOT EXISTS idx_media_files_item_key ON media_files(item_key);
    CREATE INDEX IF NOT EXISTS idx_media_files_title ON media_files(title_guess);
    CREATE INDEX IF NOT EXISTS idx_media_files_root ON media_files(root_path);
    CREATE INDEX IF NOT EXISTS idx_merge_rules_kind ON merge_rules(kind);
    "#,
    )?;

    add_column_if_missing(conn, "media_files", "music_artist", "TEXT")?;
    add_column_if_missing(conn, "media_files", "music_album", "TEXT")?;
    add_column_if_missing(conn, "media_files", "music_title", "TEXT")?;
    add_column_if_missing(conn, "media_files", "music_artist_source", "TEXT")?;
    add_column_if_missing(conn, "media_files", "series_title", "TEXT")?;
    add_column_if_missing(conn, "media_files", "series_source", "TEXT")?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing?.eq_ignore_ascii_case(column) {
            return Ok(());
        }
    }

    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn setup_database(app: &tauri::AppHandle) -> Result<DbState, Box<dyn std::error::Error>> {
    let app_data_dir = app.path().app_data_dir()?;
    fs::create_dir_all(&app_data_dir)?;
    let db_path = app_data_dir.join("media_administrator.sqlite3");
    let conn = Connection::open(&db_path)?;
    init_schema(&conn)?;
    Ok(DbState {
        conn: Mutex::new(conn),
        db_path,
        scan_running: Mutex::new(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource_variant(file_name: &str) -> ResourceVariant {
        ResourceVariant {
            id: 1,
            path: file_name.to_string(),
            file_name: file_name.to_string(),
            root_path: String::new(),
            file_size: 0,
            duration_seconds: None,
            container: None,
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            resolution: None,
            source: None,
            release_group: None,
            season_number: None,
            episode_number: None,
            title_guess: file_name.to_string(),
            media_kind: String::new(),
            music_artist: None,
            music_album: None,
            music_title: None,
            music_artist_source: None,
            series_title: None,
            series_source: None,
        }
    }

    #[test]
    fn audio_extension_with_cover_art_stays_music() {
        let mut file = resource_variant("album.flac");
        file.audio_codec = Some("FLAC".to_string());
        file.video_codec = Some("MJPEG".to_string());

        assert_eq!(media_kind_for_file(&file).as_deref(), Some("music"));
    }

    #[test]
    fn audio_extension_without_audio_stream_is_ignored() {
        let mut file = resource_variant("broken.flac");
        file.video_codec = Some("MJPEG".to_string());

        assert_eq!(media_kind_for_file(&file), None);
    }

    #[test]
    fn real_video_file_stays_video() {
        let mut file = resource_variant("movie.mkv");
        file.video_codec = Some("H.264".to_string());

        assert_eq!(media_kind_for_file(&file).as_deref(), Some("video"));
    }

    #[test]
    fn parses_bracketed_anime_episode_name() {
        let parsed = parse_name("[Nekomoe kissaten][Jigokuraku][25][1080p][JPSC].mp4");

        assert_eq!(parsed.release_group.as_deref(), Some("Nekomoe kissaten"));
        assert_eq!(parsed.title_guess, "Jigokuraku");
        assert_eq!(parsed.season_number, Some(1));
        assert_eq!(parsed.episode_number, Some(25));
    }

    #[test]
    fn parses_release_group_title_dash_episode_name() {
        let parsed = parse_name(
            "[LoliHouse] Fate strange Fake - 01 [WebRip 1080p HEVC-10bit AAC SRTx2].mkv",
        );

        assert_eq!(parsed.release_group.as_deref(), Some("LoliHouse"));
        assert_eq!(parsed.title_guess, "Fate strange Fake");
        assert_eq!(parsed.season_number, Some(1));
        assert_eq!(parsed.episode_number, Some(1));
        assert_eq!(parsed.source.as_deref(), Some("WEBRip"));
    }

    #[test]
    fn parses_artist_title_from_music_filename() {
        let parsed = infer_artist_title_from_filename("artist name - song title.flac");

        assert_eq!(
            parsed,
            Some(("artist name".to_string(), "song title".to_string()))
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db = setup_database(app.handle())?;
            app.manage(db);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_ffprobe,
            start_scan,
            list_library,
            set_merge_rules
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
