use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::UNIX_EPOCH,
};
use tauri::{Manager, State};
use walkdir::WalkDir;

struct DbState {
    conn: Mutex<Connection>,
}

#[derive(Debug, Serialize)]
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryGroup {
    key: String,
    title: String,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    variants: Vec<ResourceVariant>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanSummary {
    scanned_files: usize,
    imported_files: usize,
    skipped_files: usize,
    ffprobe_missing: bool,
    groups: Vec<LibraryGroup>,
}

#[derive(Debug, Default)]
struct MediaProbe {
    duration_seconds: Option<f64>,
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(Debug)]
struct ParsedName {
    title_guess: String,
    item_key: String,
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
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

const MEDIA_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "m4v", "ts", "m2ts", "webm", "mpg", "mpeg", "mp3",
    "flac", "m4a", "aac", "ogg", "opus", "wav", "ape",
];

#[tauri::command]
fn check_ffprobe() -> Result<String, String> {
    match Command::new("ffprobe").arg("-version").output() {
        Ok(output) if output.status.success() => Ok("ffprobe 已从 PATH 找到".to_string()),
        Ok(_) => Err("ffprobe 可执行文件存在，但版本检查失败".to_string()),
        Err(error) => Err(format!("未能从 PATH 调用 ffprobe：{error}")),
    }
}

#[tauri::command]
fn scan_paths(paths: Vec<String>, db: State<'_, DbState>) -> Result<ScanSummary, String> {
    if paths.is_empty() {
        return Err("至少需要选择一个目录".to_string());
    }

    let ffprobe_missing = Command::new("ffprobe").arg("-version").output().is_err();
    let mut scanned_files = 0;
    let mut imported_files = 0;
    let mut skipped_files = 0;
    let conn = db
        .conn
        .lock()
        .map_err(|_| "数据库连接被占用，稍后再试".to_string())?;

    for root in paths {
        let root_path = PathBuf::from(&root);
        if !root_path.exists() {
            skipped_files += 1;
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

            scanned_files += 1;
            match analyze_file(path, &root_path, !ffprobe_missing) {
                Ok(file) => {
                    upsert_file(&conn, &file)?;
                    imported_files += 1;
                }
                Err(_) => {
                    skipped_files += 1;
                }
            }
        }
    }

    Ok(ScanSummary {
        scanned_files,
        imported_files,
        skipped_files,
        ffprobe_missing,
        groups: load_library(&conn)?,
    })
}

#[tauri::command]
fn list_library(db: State<'_, DbState>) -> Result<Vec<LibraryGroup>, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "数据库连接被占用，稍后再试".to_string())?;
    load_library(&conn)
}

fn analyze_file(path: &Path, root_path: &Path, use_ffprobe: bool) -> Result<AnalyzedFile, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let parsed = parse_name(&file_name);
    let probe = if use_ffprobe {
        probe_media(path).unwrap_or_default()
    } else {
        MediaProbe::default()
    };
    let resolution = resolution_from_probe_or_name(probe.width, probe.height, &file_name);
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
        title_guess: parsed.title_guess,
        item_key: parsed.item_key,
    })
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
        probe.container = format.format_name;
        probe.duration_seconds = format.duration.and_then(|value| value.parse::<f64>().ok());
    }

    if let Some(streams) = parsed.streams {
        for stream in streams {
            match stream.codec_type.as_deref() {
                Some("video") if probe.video_codec.is_none() => {
                    probe.video_codec = stream.codec_name.map(normalize_video_codec);
                    probe.width = stream.width;
                    probe.height = stream.height;
                    if probe.duration_seconds.is_none() {
                        probe.duration_seconds =
                            stream.duration.and_then(|value| value.parse::<f64>().ok());
                    }
                }
                Some("audio") if probe.audio_codec.is_none() => {
                    probe.audio_codec = stream.codec_name.map(normalize_audio_codec);
                }
                _ => {}
            }
        }
    }

    Ok(probe)
}

fn parse_name(file_name: &str) -> ParsedName {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name);
    let release_group = detect_release_group(stem);
    let source = detect_source(stem);
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
    let normalized_title = normalize_key(&title_guess);
    let item_key = match (season_number, episode_number) {
        (Some(season), Some(episode)) => {
            format!("episode:{normalized_title}:s{season:02}:e{episode:03}")
        }
        _ => format!("item:{normalized_title}"),
    };

    ParsedName {
        title_guess,
        item_key,
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

fn upsert_file(conn: &Connection, file: &AnalyzedFile) -> Result<(), String> {
    conn.execute(
        r#"
      INSERT INTO media_files (
        path, file_name, root_path, file_size, modified_ms, duration_seconds, container,
        video_codec, audio_codec, width, height, resolution, source, release_group,
        season_number, episode_number, title_guess, item_key
      )
      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
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
            &file.item_key
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn load_library(conn: &Connection) -> Result<Vec<LibraryGroup>, String> {
    let mut stmt = conn
        .prepare(
            r#"
      SELECT
        id, path, file_name, root_path, file_size, duration_seconds, container,
        video_codec, audio_codec, width, height, resolution, source, release_group,
        season_number, episode_number, title_guess, item_key
      FROM media_files
      ORDER BY title_guess COLLATE NOCASE, season_number, episode_number, file_name COLLATE NOCASE
      "#,
        )
        .map_err(|error| error.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(17)?,
                ResourceVariant {
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
                },
            ))
        })
        .map_err(|error| error.to_string())?;

    let mut groups: Vec<LibraryGroup> = Vec::new();
    for row in rows {
        let (key, variant) = row.map_err(|error| error.to_string())?;
        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.variants.push(variant);
        } else {
            groups.push(LibraryGroup {
                key,
                title: variant.title_guess.clone(),
                season_number: variant.season_number,
                episode_number: variant.episode_number,
                variants: vec![variant],
            });
        }
    }

    Ok(groups)
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
      created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
      updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

    CREATE INDEX IF NOT EXISTS idx_media_files_item_key ON media_files(item_key);
    CREATE INDEX IF NOT EXISTS idx_media_files_title ON media_files(title_guess);
    "#,
    )
}

fn setup_database(app: &tauri::AppHandle) -> Result<DbState, Box<dyn std::error::Error>> {
    let app_data_dir = app.path().app_data_dir()?;
    fs::create_dir_all(&app_data_dir)?;
    let db_path = app_data_dir.join("media_administrator.sqlite3");
    let conn = Connection::open(db_path)?;
    init_schema(&conn)?;
    Ok(DbState {
        conn: Mutex::new(conn),
    })
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
            scan_paths,
            list_library
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
