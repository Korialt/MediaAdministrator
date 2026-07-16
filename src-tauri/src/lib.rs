use regex::Regex;
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{webview::PageLoadEvent, AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;

struct DbState {
    db_path: PathBuf,
    scan_running: Mutex<bool>,
    stop_requested: Arc<AtomicBool>,
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
    media_kind: String,
    file_count: usize,
    total_size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MediaGroup {
    key: String,
    name: String,
    subtitle: Option<String>,
    family_name: Option<String>,
    file_count: usize,
    total_size: i64,
    source_keys: Vec<String>,
    resource_keys: Vec<String>,
    child_groups: Vec<MediaGroup>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LibraryData {
    music_directories: Vec<MediaDirectory>,
    video_directories: Vec<MediaDirectory>,
    music_artists: Vec<MediaGroup>,
    video_series: Vec<MediaGroup>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourcePage {
    files: Vec<ResourceVariant>,
    total: usize,
    offset: usize,
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceQuery {
    kind: String,
    media_kind: String,
    key: String,
    source_keys: Vec<String>,
    offset: usize,
    limit: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FailedRoot {
    path: String,
    detail: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanSummary {
    scan_id: i64,
    started_at_ms: i64,
    completed_at_ms: i64,
    duration_ms: i64,
    scanned_files: usize,
    imported_files: usize,
    skipped_files: usize,
    skipped_short_files: usize,
    recorded_directories: usize,
    ffprobe_missing: bool,
    status: String,
    failed_roots: Vec<FailedRoot>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanRun {
    id: i64,
    started_at_ms: i64,
    completed_at_ms: i64,
    duration_ms: i64,
    scanned_files: i64,
    imported_files: i64,
    skipped_files: i64,
    skipped_short_files: i64,
    recorded_directories: i64,
    ffprobe_missing: bool,
    status: String,
    error_message: Option<String>,
    failed_roots: Vec<FailedRoot>,
    paths: Vec<String>,
    excluded_paths: Vec<String>,
    created_at: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanConfig {
    paths: Vec<String>,
    excluded_paths: Vec<String>,
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
    detail: String,
    scan_started_at_ms: i64,
    current_file_started_at_ms: Option<i64>,
    updated_at_ms: i64,
    ffprobe_missing: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanSkip {
    id: i64,
    scan_id: Option<i64>,
    path: String,
    file_name: String,
    root_path: String,
    reason: String,
    detail: String,
    is_short_video: bool,
    file_size: Option<i64>,
    modified_ms: Option<i64>,
    created_at: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanFailureEvent {
    scan_id: Option<i64>,
    status: String,
    message: String,
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
    root_key: String,
    directory_path: String,
    media_kind: String,
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
    music_artist_key: Option<String>,
    series_title: Option<String>,
    series_source: Option<String>,
    series_key: Option<String>,
    family_key: Option<String>,
}

#[derive(Debug)]
enum AnalyzeSkip {
    Failed { reason: String, detail: String },
    ShortVideo,
    Stopped,
}

#[derive(Debug)]
struct RootSpec {
    path: PathBuf,
    display_path: String,
    key: String,
}

#[derive(Debug)]
struct PreparedScanConfig {
    roots: Vec<RootSpec>,
    excluded_paths: Vec<String>,
}

#[derive(Debug)]
struct DiscoveredRoot {
    root: RootSpec,
    media_files: Vec<PathBuf>,
}

#[derive(Debug)]
struct PendingSkip {
    path: PathBuf,
    root_path: PathBuf,
    root_key: String,
    reason: String,
    detail: String,
    is_short_video: bool,
}

#[derive(Debug, Default)]
struct ScanCounters {
    discovered_files: usize,
    processed_files: usize,
    imported_files: usize,
    skipped_files: usize,
    skipped_short_files: usize,
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
const SCAN_STOPPED_MESSAGE: &str = "扫描已停止";
const RESOURCE_PAGE_DEFAULT_LIMIT: usize = 200;
const RESOURCE_PAGE_MAX_LIMIT: usize = 500;
const DATABASE_SCHEMA_VERSION: i64 = 1;
static COMMAND_OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn ffprobe_command() -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("ffprobe");
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }

    #[cfg(not(windows))]
    {
        Command::new("ffprobe")
    }
}

fn command_output(
    command: &mut Command,
    stop_requested: Option<&AtomicBool>,
) -> Result<Output, String> {
    if let Some(stop_requested) = stop_requested {
        if stop_requested.load(Ordering::SeqCst) {
            return Err(SCAN_STOPPED_MESSAGE.to_string());
        }
    }

    if stop_requested.is_none() {
        return command.output().map_err(|error| error.to_string());
    }

    let output_id = COMMAND_OUTPUT_COUNTER.fetch_add(1, Ordering::SeqCst);
    let output_base = format!(
        "media_administrator_command_{}_{}_{}",
        std::process::id(),
        current_time_millis(),
        output_id
    );
    let stdout_path = std::env::temp_dir().join(format!("{output_base}.stdout"));
    let stderr_path = std::env::temp_dir().join(format!("{output_base}.stderr"));
    let stdout_file = fs::File::create(&stdout_path).map_err(|error| error.to_string())?;
    let stderr_file = fs::File::create(&stderr_path).map_err(|error| {
        let _ = fs::remove_file(&stdout_path);
        error.to_string()
    })?;

    command
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    let mut child = command.spawn().map_err(|error| {
        let _ = fs::remove_file(&stdout_path);
        let _ = fs::remove_file(&stderr_path);
        error.to_string()
    })?;

    loop {
        if let Some(stop_requested) = stop_requested {
            if stop_requested.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&stdout_path);
                let _ = fs::remove_file(&stderr_path);
                return Err(SCAN_STOPPED_MESSAGE.to_string());
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout_result = fs::read(&stdout_path);
                let stderr_result = fs::read(&stderr_path);
                let _ = fs::remove_file(&stdout_path);
                let _ = fs::remove_file(&stderr_path);
                let stdout = stdout_result.map_err(|error| error.to_string())?;
                let stderr = stderr_result.map_err(|error| error.to_string())?;
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&stdout_path);
                let _ = fs::remove_file(&stderr_path);
                return Err(error.to_string());
            }
        }
    }
}

fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn ensure_scan_not_stopped(stop_requested: &AtomicBool) -> Result<(), String> {
    if stop_requested.load(Ordering::SeqCst) {
        Err(SCAN_STOPPED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

#[tauri::command]
fn check_ffprobe() -> Result<String, String> {
    verify_ffprobe(None).map(|_| "ffprobe 已从 PATH 找到".to_string())
}

#[tauri::command]
fn start_scan(
    paths: Vec<String>,
    excluded_paths: Option<Vec<String>>,
    app: AppHandle,
    db: State<'_, DbState>,
) -> Result<(), String> {
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

    db.stop_requested.store(false, Ordering::SeqCst);
    let db_path = db.db_path.clone();
    let app_handle = app.clone();
    let excluded_paths = excluded_paths.unwrap_or_default();
    let stop_requested = Arc::clone(&db.stop_requested);
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_scan(
            paths,
            excluded_paths,
            &db_path,
            &stop_requested,
            Some(&app_handle),
        );

        // 完成事件发出前先释放运行状态，避免前端立即重扫时收到“已有任务”。
        let state = app_handle.state::<DbState>();
        state.stop_requested.store(false, Ordering::SeqCst);
        if let Ok(mut running) = state.scan_running.lock() {
            *running = false;
        }

        match result {
            Ok(summary) => {
                let _ = app_handle.emit("scan-complete", summary);
            }
            Err(error) => {
                let _ = app_handle.emit("scan-error", error);
            }
        }
    });

    Ok(())
}

#[tauri::command]
fn stop_scan(db: State<'_, DbState>) -> Result<(), String> {
    let running = db
        .scan_running
        .lock()
        .map_err(|_| "扫描状态被占用，稍后再试".to_string())?;
    if !*running {
        return Err("当前没有正在运行的扫描任务".to_string());
    }
    db.stop_requested.store(true, Ordering::SeqCst);
    Ok(())
}

fn run_scan(
    paths: Vec<String>,
    excluded_paths: Vec<String>,
    db_path: &Path,
    stop_requested: &AtomicBool,
    app: Option<&AppHandle>,
) -> Result<ScanSummary, ScanFailureEvent> {
    let scan_started_at_ms = current_time_millis();
    let prepared =
        prepare_scan_config(paths, excluded_paths).map_err(|message| ScanFailureEvent {
            scan_id: None,
            status: "failed".to_string(),
            message,
        })?;
    let scan_paths: Vec<String> = prepared
        .roots
        .iter()
        .map(|root| root.display_path.clone())
        .collect();
    let conn = open_connection(db_path).map_err(|message| ScanFailureEvent {
        scan_id: None,
        status: "failed".to_string(),
        message,
    })?;
    let scan_id = create_scan_run(
        &conn,
        scan_started_at_ms,
        &scan_paths,
        &prepared.excluded_paths,
    )
    .map_err(|message| ScanFailureEvent {
        scan_id: None,
        status: "failed".to_string(),
        message,
    })?;
    let mut counters = ScanCounters::default();
    let mut last_emit = Instant::now();

    let execution = execute_scan(
        &conn,
        scan_id,
        prepared,
        stop_requested,
        app,
        scan_started_at_ms,
        &mut counters,
        &mut last_emit,
    );

    match execution {
        Ok((failed_roots, completed_roots, recorded_directories)) => {
            let completed_at_ms = current_time_millis();
            let status = if completed_roots == 0 && !failed_roots.is_empty() {
                "failed"
            } else if failed_roots.is_empty() {
                "completed"
            } else {
                "partial"
            };
            let error_message = (status == "failed").then(|| "所有扫描根目录均不可用".to_string());
            let summary = ScanSummary {
                scan_id,
                started_at_ms: scan_started_at_ms,
                completed_at_ms,
                duration_ms: (completed_at_ms - scan_started_at_ms).max(0),
                scanned_files: counters.processed_files,
                imported_files: counters.imported_files,
                skipped_files: counters.skipped_files,
                skipped_short_files: counters.skipped_short_files,
                recorded_directories,
                ffprobe_missing: false,
                status: status.to_string(),
                failed_roots: failed_roots.clone(),
            };
            finish_scan_run(&conn, &summary, error_message.as_deref()).map_err(|message| {
                ScanFailureEvent {
                    scan_id: Some(scan_id),
                    status: "failed".to_string(),
                    message,
                }
            })?;

            Ok(summary)
        }
        Err(message) => {
            let status = if message.starts_with(SCAN_STOPPED_MESSAGE) {
                "stopped"
            } else {
                "failed"
            };
            let ffprobe_missing = message.starts_with("ffprobe 预检失败");
            let completed_at_ms = current_time_millis();
            let (recorded_directories, message) = match count_recorded_directories(&conn) {
                Ok(count) => (count, message),
                Err(error) => (0, format!("{message}；无法统计现有目录：{error}")),
            };
            let summary = ScanSummary {
                scan_id,
                started_at_ms: scan_started_at_ms,
                completed_at_ms,
                duration_ms: (completed_at_ms - scan_started_at_ms).max(0),
                scanned_files: counters.processed_files,
                imported_files: counters.imported_files,
                skipped_files: counters.skipped_files,
                skipped_short_files: counters.skipped_short_files,
                recorded_directories,
                ffprobe_missing,
                status: status.to_string(),
                failed_roots: Vec::new(),
            };
            let message = finish_scan_run(&conn, &summary, Some(&message))
                .err()
                .map(|error| format!("{message}；扫描历史写入失败：{error}"))
                .unwrap_or(message);
            Err(ScanFailureEvent {
                scan_id: Some(scan_id),
                status: status.to_string(),
                message,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_scan(
    conn: &Connection,
    scan_id: i64,
    prepared: PreparedScanConfig,
    stop_requested: &AtomicBool,
    app: Option<&AppHandle>,
    scan_started_at_ms: i64,
    counters: &mut ScanCounters,
    last_emit: &mut Instant,
) -> Result<(Vec<FailedRoot>, usize, usize), String> {
    ensure_scan_not_stopped(stop_requested)?;
    verify_ffprobe(Some(stop_requested)).map_err(|error| format!("ffprobe 预检失败：{error}"))?;
    let excluded_roots = build_excluded_roots(&prepared.excluded_paths);
    let mut discovered_roots = Vec::new();
    let mut failed_roots = Vec::new();

    emit_scan_progress(
        app,
        "discovering",
        counters,
        None,
        None,
        "准备统计目录媒体文件",
        scan_started_at_ms,
        None,
        last_emit,
        true,
    );

    for root in prepared.roots {
        ensure_scan_not_stopped(stop_requested)?;
        if !root.path.is_dir() {
            failed_roots.push(FailedRoot {
                path: root.display_path.clone(),
                detail: "目录不存在、不可访问或不是目录，已保留原有索引".to_string(),
            });
            emit_scan_progress(
                app,
                "discovering",
                counters,
                None,
                Some(root.display_path),
                "根目录不可用，已保留原有索引",
                scan_started_at_ms,
                None,
                last_emit,
                true,
            );
            continue;
        }

        let mut media_files = Vec::new();
        let mut traversal_errors = Vec::new();
        let walker = WalkDir::new(&root.path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_excluded_path(entry.path(), &excluded_roots));

        for entry in walker {
            ensure_scan_not_stopped(stop_requested)?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    traversal_errors.push(error.to_string());
                    continue;
                }
            };
            let path = entry.path();
            if entry.file_type().is_file() && is_media_file(path) {
                counters.discovered_files += 1;
                media_files.push(path.to_path_buf());
                emit_scan_progress(
                    app,
                    "discovering",
                    counters,
                    None,
                    Some(path.to_string_lossy().to_string()),
                    "发现媒体文件",
                    scan_started_at_ms,
                    None,
                    last_emit,
                    false,
                );
            }
        }

        if traversal_errors.is_empty() {
            discovered_roots.push(DiscoveredRoot { root, media_files });
        } else {
            counters.discovered_files = counters.discovered_files.saturating_sub(media_files.len());
            let extra_count = traversal_errors.len().saturating_sub(1);
            let detail = if extra_count == 0 {
                format!("目录遍历不完整：{}；已保留原有索引", traversal_errors[0])
            } else {
                format!(
                    "目录遍历不完整：{}；另有 {extra_count} 个错误；已保留原有索引",
                    traversal_errors[0]
                )
            };
            failed_roots.push(FailedRoot {
                path: root.display_path.clone(),
                detail,
            });
            emit_scan_progress(
                app,
                "discovering",
                counters,
                None,
                Some(root.display_path),
                "目录遍历不完整，已保留原有索引",
                scan_started_at_ms,
                None,
                last_emit,
                true,
            );
        }
    }

    let total_files = discovered_roots
        .iter()
        .map(|root| root.media_files.len())
        .sum::<usize>();
    emit_scan_progress(
        app,
        "processing",
        counters,
        Some(total_files),
        None,
        "开始分析媒体文件",
        scan_started_at_ms,
        None,
        last_emit,
        true,
    );

    let completed_roots = discovered_roots.len();
    for discovered_root in discovered_roots {
        let mut analyzed_files = Vec::new();
        let mut pending_skips = Vec::new();

        for path in discovered_root.media_files {
            if stop_requested.load(Ordering::SeqCst) {
                persist_pending_skips_on_stop(conn, scan_id, &pending_skips)?;
                return Err(SCAN_STOPPED_MESSAGE.to_string());
            }
            counters.processed_files += 1;
            let current_path = path.to_string_lossy().to_string();
            let current_file_started_at_ms = current_time_millis();
            emit_scan_progress(
                app,
                "processing",
                counters,
                Some(total_files),
                Some(current_path.clone()),
                "正在调用 ffprobe 分析媒体流",
                scan_started_at_ms,
                Some(current_file_started_at_ms),
                last_emit,
                true,
            );

            match analyze_file(&path, &discovered_root.root.path, stop_requested) {
                Ok(file) => analyzed_files.push(file),
                Err(AnalyzeSkip::ShortVideo) => {
                    counters.skipped_files += 1;
                    counters.skipped_short_files += 1;
                    pending_skips.push(PendingSkip {
                        path: path.clone(),
                        root_path: discovered_root.root.path.clone(),
                        root_key: discovered_root.root.key.clone(),
                        reason: "short_video".to_string(),
                        detail: "短视频少于 5 分钟，已从媒体库过滤".to_string(),
                        is_short_video: true,
                    });
                }
                Err(AnalyzeSkip::Failed { reason, detail }) => {
                    counters.skipped_files += 1;
                    pending_skips.push(PendingSkip {
                        path: path.clone(),
                        root_path: discovered_root.root.path.clone(),
                        root_key: discovered_root.root.key.clone(),
                        reason,
                        detail,
                        is_short_video: false,
                    });
                }
                Err(AnalyzeSkip::Stopped) => {
                    persist_pending_skips_on_stop(conn, scan_id, &pending_skips)?;
                    return Err(SCAN_STOPPED_MESSAGE.to_string());
                }
            }

            emit_scan_progress(
                app,
                "processing",
                counters,
                Some(total_files),
                Some(current_path),
                "当前文件处理完成",
                scan_started_at_ms,
                None,
                last_emit,
                false,
            );
        }

        if stop_requested.load(Ordering::SeqCst) {
            persist_pending_skips_on_stop(conn, scan_id, &pending_skips)?;
            return Err(SCAN_STOPPED_MESSAGE.to_string());
        }
        let imported_count = analyzed_files.len();
        commit_root_scan(
            conn,
            scan_id,
            &discovered_root.root,
            &analyzed_files,
            &pending_skips,
        )?;
        counters.imported_files += imported_count;
        emit_scan_progress(
            app,
            "processing",
            counters,
            Some(total_files),
            Some(discovered_root.root.display_path),
            "根目录索引已提交",
            scan_started_at_ms,
            None,
            last_emit,
            true,
        );
    }

    ensure_scan_not_stopped(stop_requested)?;
    let recorded_directories = count_recorded_directories(conn)?;
    Ok((failed_roots, completed_roots, recorded_directories))
}

fn verify_ffprobe(stop_requested: Option<&AtomicBool>) -> Result<(), String> {
    let mut command = ffprobe_command();
    command.arg("-version");
    let output = command_output(&mut command, stop_requested)
        .map_err(|error| format!("未能从 PATH 调用 ffprobe：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffprobe 版本检查返回非零状态：{}",
            compact_process_output(&output.stderr)
        ))
    }
}

fn compact_process_output(bytes: &[u8]) -> String {
    let value = String::from_utf8_lossy(bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if value.is_empty() {
        "没有错误输出".to_string()
    } else {
        value.chars().take(600).collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_scan_progress(
    app: Option<&AppHandle>,
    phase: &str,
    counters: &ScanCounters,
    total_files: Option<usize>,
    current_path: Option<String>,
    detail: &str,
    scan_started_at_ms: i64,
    current_file_started_at_ms: Option<i64>,
    last_emit: &mut Instant,
    force: bool,
) {
    maybe_emit_scan_progress(
        app,
        phase,
        counters.discovered_files,
        counters.processed_files,
        counters.imported_files,
        counters.skipped_files,
        counters.skipped_short_files,
        total_files,
        current_path,
        detail,
        scan_started_at_ms,
        current_file_started_at_ms,
        false,
        last_emit,
        force,
    );
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
    detail: &str,
    scan_started_at_ms: i64,
    current_file_started_at_ms: Option<i64>,
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
            detail: detail.to_string(),
            scan_started_at_ms,
            current_file_started_at_ms,
            updated_at_ms: current_time_millis(),
            ffprobe_missing,
        };
        let _ = app.emit("scan-progress", progress);
    }

    *last_emit = Instant::now();
}

#[tauri::command]
async fn list_library(
    query: Option<String>,
    db: State<'_, DbState>,
) -> Result<LibraryData, String> {
    let db_path = db.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_connection(&db_path)?;
        load_library(&conn, query.as_deref())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_resources(
    request: ResourceQuery,
    db: State<'_, DbState>,
) -> Result<ResourcePage, String> {
    let db_path = db.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_connection(&db_path)?;
        load_resource_page(&conn, request)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_scan_skips(scan_id: i64, db: State<'_, DbState>) -> Result<Vec<ScanSkip>, String> {
    if scan_id <= 0 {
        return Err("扫描记录编号无效".to_string());
    }
    let db_path = db.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_connection(&db_path)?;
        load_scan_skips(&conn, scan_id, false)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_scan_history(db: State<'_, DbState>) -> Result<Vec<ScanRun>, String> {
    let db_path = db.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_connection(&db_path)?;
        load_scan_history(&conn, 50)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_last_scan_config(db: State<'_, DbState>) -> Result<ScanConfig, String> {
    let db_path = db.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_connection(&db_path)?;
        load_last_scan_config(&conn)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn set_merge_rules(
    request: MergeRequest,
    db: State<'_, DbState>,
) -> Result<LibraryData, String> {
    if !matches!(
        request.kind.as_str(),
        "music_artist" | "video_series" | "video_family"
    ) {
        return Err("不支持的合并类型".to_string());
    }
    {
        let running = db
            .scan_running
            .lock()
            .map_err(|_| "扫描状态被占用，稍后再试".to_string())?;
        if *running {
            return Err("扫描期间不能修改分类规则".to_string());
        }
    }

    let db_path = db.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let expected_source_prefix = if request.kind == "music_artist" {
            "music_artist:"
        } else {
            "video_series:"
        };
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
        if source_keys.len() > 400
            || source_keys
                .iter()
                .any(|key| key.len() > 512 || !key.starts_with(expected_source_prefix))
        {
            return Err("合并来源数量或格式无效".to_string());
        }

        let target_name = request.target_name.trim().to_string();
        if target_name.chars().count() > 240 {
            return Err("目标名称不能超过 240 个字符".to_string());
        }
        let conn = open_connection(&db_path)?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;

        if target_name.is_empty() {
            for source_key in source_keys {
                tx.execute(
                    "DELETE FROM merge_rules WHERE kind = ?1 AND source_key = ?2",
                    params![&request.kind, source_key],
                )
                .map_err(|error| error.to_string())?;
            }
        } else {
            for source_key in source_keys {
                tx.execute(
                    r#"
                    INSERT INTO merge_rules (kind, source_key, target_name)
                    VALUES (?1, ?2, ?3)
                    ON CONFLICT(kind, source_key) DO UPDATE SET
                      target_name = excluded.target_name,
                      updated_at = CURRENT_TIMESTAMP
                    "#,
                    params![&request.kind, source_key, &target_name],
                )
                .map_err(|error| error.to_string())?;
            }
        }
        tx.commit().map_err(|error| error.to_string())?;

        load_library(&conn, None)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn analyze_file(
    path: &Path,
    root_path: &Path,
    stop_requested: &AtomicBool,
) -> Result<AnalyzedFile, AnalyzeSkip> {
    if stop_requested.load(Ordering::SeqCst) {
        return Err(AnalyzeSkip::Stopped);
    }

    let metadata = fs::metadata(path).map_err(|error| AnalyzeSkip::Failed {
        reason: "metadata_error".to_string(),
        detail: format!("无法读取文件元数据：{error}"),
    })?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| AnalyzeSkip::Failed {
            reason: "invalid_path".to_string(),
            detail: "文件路径中缺少文件名".to_string(),
        })?;
    let parsed = parse_name(&file_name);
    let probe = probe_media(path, stop_requested).map_err(|error| {
        if error == SCAN_STOPPED_MESSAGE {
            AnalyzeSkip::Stopped
        } else {
            AnalyzeSkip::Failed {
                reason: "ffprobe_error".to_string(),
                detail: error,
            }
        }
    })?;

    if !probe.has_video && !probe.has_audio {
        return Err(AnalyzeSkip::Failed {
            reason: "no_media_stream".to_string(),
            detail: "ffprobe 未检测到有效的视频流或音频流".to_string(),
        });
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
    let directory_path = path
        .parent()
        .map(|directory| directory.to_string_lossy().to_string())
        .unwrap_or_else(|| root_string.clone());
    let media_kind = if probe.has_video { "video" } else { "music" }.to_string();
    let music_artist_key = music_artist
        .as_deref()
        .map(|artist| group_source_key("music_artist", artist));
    let series_key = series_title
        .as_deref()
        .map(|series| group_source_key("video_series", series));
    let family_key = series_title
        .as_deref()
        .and_then(|series| infer_series_family(path, root_path, series))
        .map(|family| group_source_key("video_series", &family));
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);

    Ok(AnalyzedFile {
        path: path_string,
        file_name,
        root_path: root_string,
        root_key: normalized_path_string(root_path),
        directory_path,
        media_kind,
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
        music_artist_key,
        series_title,
        series_source,
        series_key,
        family_key,
    })
}

fn is_short_video(probe: &MediaProbe) -> bool {
    probe.has_video
        && probe
            .duration_seconds
            .map(|duration| duration < MIN_VIDEO_SECONDS)
            .unwrap_or(false)
}

fn probe_media(path: &Path, stop_requested: &AtomicBool) -> Result<MediaProbe, String> {
    let mut command = ffprobe_command();
    command
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path);
    let output = command_output(&mut command, Some(stop_requested))?;

    if !output.status.success() {
        return Err(format!(
            "ffprobe 返回非零状态：{}",
            compact_process_output(&output.stderr)
        ));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("ffprobe JSON 解析失败：{error}"))?;
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

    if let Some((artist, _)) = filename_artist_title {
        return (Some(artist), album, title, Some("filename".to_string()));
    }

    if let Some(artist) = infer_artist_from_directory(path, root_path) {
        return (Some(artist), album, title, Some("directory".to_string()));
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
    let meaningful_components: Vec<&String> = components
        .iter()
        .filter(|value| !is_generic_music_directory(value))
        .collect();
    let candidate = match meaningful_components.len() {
        0 => None,
        1 => meaningful_components.last().copied(),
        _ => meaningful_components
            .get(meaningful_components.len() - 2)
            .copied(),
    }?;
    clean_group_name(candidate).filter(|value| !is_generic_music_directory(value))
}

fn infer_album_from_directory(path: &Path, root_path: &Path) -> Option<String> {
    let components = relative_parent_components(path, root_path);
    let meaningful_components: Vec<&String> = components
        .iter()
        .filter(|value| !is_generic_music_directory(value))
        .collect();
    if meaningful_components.len() < 2 {
        return None;
    }
    meaningful_components
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
    (is_plausible_filename_artist(&artist) && !title.is_empty()).then_some((artist, title))
}

fn is_plausible_filename_artist(value: &str) -> bool {
    let key = normalize_key(value);
    !key.is_empty() && key.chars().any(char::is_alphabetic) && !is_generic_music_directory(value)
}

fn infer_video_series(
    path: &Path,
    root_path: &Path,
    parsed: &ParsedName,
) -> (Option<String>, Option<String>) {
    let parsed_title = normalize_series_title(&parsed.title_guess);
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    if is_confident_filename_title(&file_name, &parsed_title) {
        return (Some(parsed_title), Some("filename".to_string()));
    }

    if let Some(series) = infer_series_from_directory(path, root_path) {
        return (Some(series), Some("directory".to_string()));
    }

    if parsed.title_guess != "未命名资源" {
        return (
            Some(normalize_series_title(&parsed.title_guess)),
            Some("filename".to_string()),
        );
    }

    (Some("未识别系列".to_string()), Some("unknown".to_string()))
}

fn infer_series_from_directory(path: &Path, root_path: &Path) -> Option<String> {
    let components = relative_parent_components(path, root_path);
    for component in components.iter().rev() {
        if let Some(candidate) = clean_directory_title(component) {
            if !is_generic_video_directory(&candidate) && !is_generated_directory_title(&candidate)
            {
                return Some(candidate);
            }
        }
    }

    root_path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(clean_directory_title)
        .filter(|candidate| {
            !is_generic_video_directory(candidate) && !is_generated_directory_title(candidate)
        })
}

fn infer_series_from_file_name(file_name: &str) -> Option<String> {
    let parsed = parse_name(file_name);
    let title = normalize_series_title(&parsed.title_guess);
    is_confident_filename_title(file_name, &title).then_some(title)
}

fn is_confident_filename_title(file_name: &str, title: &str) -> bool {
    if !is_detected_title(title) || is_generic_filename_title(title) {
        return false;
    }

    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name);
    let bracketed_title = Regex::new(r"^\[[^\]]+\]\s*\[[^\]]+\]")
        .expect("valid bracketed title regex")
        .is_match(stem);
    let explicit_episode = detect_season_episode(stem).is_some()
        || detect_anime_episode(stem).is_some()
        || detect_leading_episode(stem).is_none();
    bracketed_title || explicit_episode
}

fn is_generic_filename_title(value: &str) -> bool {
    matches!(
        normalize_key(value).as_str(),
        "episode"
            | "ep"
            | "special"
            | "specials"
            | "sp"
            | "ova"
            | "oad"
            | "ncop"
            | "nced"
            | "op"
            | "ed"
            | "pv"
            | "cm"
            | "trailer"
            | "preview"
            | "sample"
            | "menu"
            | "bonus"
            | "extra"
            | "extras"
    )
}

fn clean_directory_title(value: &str) -> Option<String> {
    let bracketed = parse_bracketed_anime_name(value, None, None).map(|parsed| parsed.title_guess);
    if bracketed.as_deref().is_some_and(is_detected_title) {
        return bracketed.map(|title| normalize_series_title(&title));
    }

    let without_group = strip_release_group_prefix(value);
    let source = if without_group.trim().is_empty() {
        value
    } else {
        &without_group
    };
    let leading_order = Regex::new(r"^\s*\d{1,3}\s*[-._]\s*").expect("valid directory order regex");
    let no_order = leading_order.replace(source, "");
    let normalized = normalize_series_title(&no_order);
    let installment_suffix = Regex::new(
        r"(?i)\s+-?\s*(?:s\d{1,3}|season\s*\d{1,3}|\d+(?:st|nd|rd|th)\s+season|part\s*\d{1,3}|cour\s*\d{1,3}|第\s*\d{1,3}\s*[季期])\s*$",
    )
    .expect("valid directory installment regex");
    // 发布标签可能位于季数后方，先清理技术信息才能稳定识别末尾的分季标记。
    let without_installment = installment_suffix.replace(&normalized, "");
    let cleaned = normalize_series_title(&without_installment);
    is_detected_title(&cleaned).then_some(cleaned)
}

fn should_replace_stored_series(stored_series: &str, filename_series: &str) -> bool {
    !filename_series.trim().is_empty()
        && normalize_key(stored_series) != normalize_key(filename_series)
        && is_generated_directory_title(stored_series)
}

fn is_generated_directory_title(value: &str) -> bool {
    let key = normalize_key(value);
    let digit_count = key.chars().filter(|ch| ch.is_ascii_digit()).count();
    !key.is_empty() && digit_count == key.len() && matches!(key.len(), 6 | 8)
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
    let key = normalize_key(value);
    if Regex::new(r"^(?:cd|disc|disk|vol|volume)-?\d{1,3}$")
        .expect("valid music disc directory regex")
        .is_match(&key)
    {
        return true;
    }

    matches!(
        key.as_str(),
        "ost" | "music" | "audio" | "songs" | "soundtrack" | "soundtracks" | "cd" | "disc"
    )
}

fn is_generic_video_directory(value: &str) -> bool {
    let key = normalize_key(value);
    if Regex::new(r"^(?:s|season-?)(?:\d{1,3})$")
        .expect("valid season directory regex")
        .is_match(&key)
        || Regex::new(r"^(?:cd|disc|disk|vol|volume)-?\d{1,3}$")
            .expect("valid disc directory regex")
            .is_match(&key)
        || Regex::new(r"^(?:s\d{1,3}e\d{1,4}|\d{1,3}x\d{1,4}|(?:e|ep|episode)-?\d{1,4})$")
            .expect("valid episode directory regex")
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
            | "anime"
            | "animation"
            | "tv"
            | "tv-series"
            | "series"
            | "movie"
            | "movies"
    )
}

fn normalize_series_title(value: &str) -> String {
    let cleaned = clean_title(value);
    if cleaned == "未命名资源" {
        return cleaned;
    }

    let episode_suffix =
        Regex::new(r"(?i)\s+-\s*\d{1,4}(?:v\d+)?$").expect("valid episode suffix regex");
    let without_episode = episode_suffix.replace(&cleaned, " ");
    let normalized = without_episode
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(&['-', '.', '_', ' '][..])
        .to_string();

    if normalized.is_empty() {
        "未命名资源".to_string()
    } else {
        canonicalize_ascii_title(&normalized)
    }
}

fn canonicalize_ascii_title(value: &str) -> String {
    let small_words = [
        "a", "an", "and", "as", "at", "for", "in", "no", "of", "on", "or", "the", "to",
    ];
    value
        .split_whitespace()
        .enumerate()
        .map(|(index, word)| {
            if !word.chars().any(|ch| ch.is_ascii_alphabetic()) {
                return word.to_string();
            }
            if word
                .chars()
                .any(|ch| !ch.is_ascii_alphabetic() && ch != '-' && ch != '\'')
            {
                return word.to_string();
            }
            if word.chars().all(|ch| ch.is_ascii_uppercase()) && word.len() <= 5 {
                return word.to_string();
            }

            word.split('-')
                .map(|part| {
                    let lower = part.to_ascii_lowercase();
                    if index > 0 && small_words.contains(&lower.as_str()) {
                        return lower;
                    }

                    let mut chars = lower.chars();
                    match chars.next() {
                        Some(first) => {
                            format!(
                                "{}{}",
                                first.to_ascii_uppercase(),
                                chars.collect::<String>()
                            )
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn infer_series_family(path: &Path, root_path: &Path, series: &str) -> Option<String> {
    if let Some(family) = infer_series_family_title(series) {
        return Some(family);
    }

    let series_key = normalize_key(series);
    let candidates: Vec<String> = relative_parent_components(path, root_path)
        .into_iter()
        .filter_map(|component| clean_directory_title(&component))
        .filter(|candidate| {
            !is_generic_video_directory(candidate) && !is_generated_directory_title(candidate)
        })
        .collect();
    let series_index = candidates
        .iter()
        .rposition(|candidate| normalize_key(candidate) == series_key)?;
    let parent = candidates.get(series_index.checked_sub(1)?)?;
    let parent_key = normalize_key(parent);
    let shares_series_prefix = series_key.starts_with(&format!("{parent_key}-"));
    (parent_key != series_key && shares_series_prefix).then(|| parent.clone())
}

fn infer_series_family_title(series: &str) -> Option<String> {
    let key = normalize_key(series);
    if key != "monogatari" && key.contains("monogatari") {
        return Some("Monogatari".to_string());
    }

    None
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
    let leading_episode = if season_episode.is_none() && anime_episode.is_none() {
        detect_leading_episode(&without_group)
    } else {
        None
    };

    let (title_source, season_number, episode_number) =
        if let Some((start, season, episode)) = season_episode {
            (&without_group[..start], Some(season), Some(episode))
        } else if let Some((start, episode)) = anime_episode {
            (&without_group[..start], Some(1), Some(episode))
        } else if let Some(episode) = leading_episode {
            ("", Some(1), Some(episode))
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
        Regex::new(r"(?i)(?:^|[\s._-])(\d{1,2})x(\d{1,3})(?:\D|$)").ok()?,
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
        Regex::new(r"(?i)\s+-\s*(\d{1,4})(?:v\d+)?\b").ok()?,
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

fn detect_leading_episode(value: &str) -> Option<i64> {
    let pattern = Regex::new(r"(?i)^\s*(\d{1,3})(?:v\d+)?(?:$|([\s._\-\[\(].*)$)").ok()?;
    let caps = pattern.captures(value)?;
    let episode_text = caps.get(1)?.as_str();
    let episode = episode_text.parse::<i64>().ok()?;
    if !(1..=999).contains(&episode) {
        return None;
    }

    let rest = caps.get(2).map(|value| value.as_str()).unwrap_or_default();
    let has_leading_zero = episode_text.len() > 1 && episode_text.starts_with('0');
    let rest_is_only_technical = clean_title(rest) == "未命名资源";

    (has_leading_zero || rest_is_only_technical).then_some(episode)
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
    let normalized = value.replace(['.', '_'], " ").to_uppercase();
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
        r"(?i)\b(3840x2160|1920x1080|1280x720|2160p|1080p|720p|480p|4k|8k|bd[- ]?box|web[- ]?dl|webrip|bdrip|blu[- ]?ray|bdremux|remux|hdtv|x264|x265|h\.?264|h\.?265|avc|hevc|av1|aacx\d+|aac|flac|truehd|dts|12[- ]?bit|10[- ]?bit|8[- ]?bit|hdr|dv)\b",
    )
    .expect("valid tag regex");
    let bracket_pattern = Regex::new(r"[\[\(][^\]\)]*[\]\)]").expect("valid bracket regex");
    let technical_suffix_pattern = Regex::new(
        r"(?i)\s+-\s+(?:bd[- ]?box|bd|blu[- ]?ray|dvd|web[- ]?dl|webrip|remux|disc\s*\d+|vol\.?\s*\d+).*$",
    )
    .expect("valid technical suffix regex");
    let unclosed_technical_pattern = Regex::new(
        r"(?i)\s*[\[\(](?:bd|blu[- ]?ray|dvd|web[- ]?dl|webrip|remux|hdtv|uhd|3840x2160|1920x1080|1280x720|2160p|1080p|720p|avc|hevc|x264|x265|aac|flac|dts|12[- ]?bit|10[- ]?bit).*$",
    )
    .expect("valid unclosed technical regex");
    let separated = value.replace(['.', '_'], " ");
    let no_technical_suffix = technical_suffix_pattern.replace(&separated, " ");
    let no_tags = tag_pattern.replace_all(&no_technical_suffix, " ");
    let no_brackets = bracket_pattern.replace_all(&no_tags, " ");
    let no_unclosed_technical = unclosed_technical_pattern.replace(&no_brackets, " ");
    let cleaned = no_unclosed_technical
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

fn prepare_scan_config(
    paths: Vec<String>,
    excluded_paths: Vec<String>,
) -> Result<PreparedScanConfig, String> {
    let mut roots: Vec<RootSpec> = Vec::new();
    for value in paths {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let input_path = PathBuf::from(trimmed);
        let absolute_path = if input_path.is_absolute() {
            input_path
        } else {
            std::env::current_dir()
                .map_err(|error| format!("无法解析扫描目录 {trimmed}：{error}"))?
                .join(input_path)
        };
        let path = fs::canonicalize(&absolute_path).unwrap_or(absolute_path);
        let key = normalized_path_string(&path);
        if roots
            .iter()
            .any(|root| root.key == key || is_descendant_key(&key, &root.key))
        {
            continue;
        }
        roots.retain(|root| !is_descendant_key(&root.key, &key));
        roots.push(RootSpec {
            display_path: display_path_string(&path),
            path,
            key,
        });
    }
    if roots.is_empty() {
        return Err("至少需要选择一个有效目录".to_string());
    }

    let mut normalized_exclusions: Vec<(String, String)> = Vec::new();
    for value in excluded_paths {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let input_path = PathBuf::from(trimmed);
        let absolute_path = if input_path.is_absolute() {
            input_path
        } else {
            std::env::current_dir()
                .map_err(|error| format!("无法解析排除目录 {trimmed}：{error}"))?
                .join(input_path)
        };
        let path = fs::canonicalize(&absolute_path).unwrap_or(absolute_path);
        let key = normalized_path_string(&path);
        if roots.iter().any(|root| root.key == key) {
            return Err(format!(
                "排除目录不能与扫描根目录相同：{}",
                display_path_string(&path)
            ));
        }
        if !roots.iter().any(|root| is_descendant_key(&key, &root.key)) {
            return Err(format!(
                "排除目录必须位于所选扫描目录内：{}",
                display_path_string(&path)
            ));
        }
        if normalized_exclusions
            .iter()
            .any(|(_, existing)| existing == &key || is_descendant_key(&key, existing))
        {
            continue;
        }
        normalized_exclusions.retain(|(_, existing)| !is_descendant_key(existing, &key));
        normalized_exclusions.push((display_path_string(&path), key));
    }

    Ok(PreparedScanConfig {
        roots,
        excluded_paths: normalized_exclusions
            .into_iter()
            .map(|(path, _)| path)
            .collect(),
    })
}

fn is_descendant_key(candidate: &str, parent: &str) -> bool {
    candidate
        .strip_prefix(parent)
        .map(|remaining| remaining.starts_with('/'))
        .unwrap_or(false)
}

fn display_path_string(path: &Path) -> String {
    let value = path.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        if let Some(network_path) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{network_path}");
        }
        if let Some(local_path) = value.strip_prefix(r"\\?\") {
            return local_path.to_string();
        }
    }
    value
}

fn build_excluded_roots(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| {
            let trimmed = path.trim();
            (!trimmed.is_empty()).then(|| normalized_path_string(Path::new(trimmed)))
        })
        .collect()
}

fn is_excluded_path(path: &Path, excluded_roots: &[String]) -> bool {
    if excluded_roots.is_empty() {
        return false;
    }

    let candidate = normalized_path_string(path);
    excluded_roots.iter().any(|root| {
        candidate == *root
            || candidate
                .strip_prefix(root)
                .map(|remaining| remaining.starts_with('/'))
                .unwrap_or(false)
    })
}

fn normalized_path_string(path: &Path) -> String {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut value = display_path_string(&normalized).replace('\\', "/");
    while value.ends_with('/') && value.len() > 1 {
        value.pop();
    }
    #[cfg(windows)]
    {
        value.make_ascii_lowercase();
    }
    value
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
        path, file_name, root_path, root_key, directory_path, media_kind, file_size,
        modified_ms, duration_seconds, container, video_codec, audio_codec, width, height,
        resolution, source, release_group, season_number, episode_number, title_guess,
        item_key, music_artist, music_album, music_title, music_artist_source,
        music_artist_key, series_title, series_source, series_key, family_key
      )
      VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
        ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30
      )
      ON CONFLICT(path) DO UPDATE SET
        file_name = excluded.file_name,
        root_path = excluded.root_path,
        root_key = excluded.root_key,
        directory_path = excluded.directory_path,
        media_kind = excluded.media_kind,
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
        music_artist_key = excluded.music_artist_key,
        series_title = excluded.series_title,
        series_source = excluded.series_source,
        series_key = excluded.series_key,
        family_key = excluded.family_key,
        updated_at = CURRENT_TIMESTAMP
      "#,
        params![
            &file.path,
            &file.file_name,
            &file.root_path,
            &file.root_key,
            &file.directory_path,
            &file.media_kind,
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
            file.music_artist_key.as_deref(),
            file.series_title.as_deref(),
            file.series_source.as_deref(),
            file.series_key.as_deref(),
            file.family_key.as_deref()
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn commit_root_scan(
    conn: &Connection,
    scan_id: i64,
    root: &RootSpec,
    files: &[AnalyzedFile],
    skips: &[PendingSkip],
) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    tx.execute(
        "DELETE FROM media_files WHERE root_key = ?1",
        params![&root.key],
    )
    .map_err(|error| error.to_string())?;
    for file in files {
        upsert_file(&tx, file)?;
    }
    for skip in skips {
        record_scan_skip(&tx, scan_id, skip)?;
    }
    tx.commit().map_err(|error| error.to_string())
}

fn persist_pending_skips_on_stop(
    conn: &Connection,
    scan_id: i64,
    skips: &[PendingSkip],
) -> Result<(), String> {
    if skips.is_empty() {
        return Ok(());
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("{SCAN_STOPPED_MESSAGE}；无法保存已产生的跳过明细：{error}"))?;
    for skip in skips {
        record_scan_skip(&tx, scan_id, skip).map_err(|error| {
            format!("{SCAN_STOPPED_MESSAGE}；无法保存已产生的跳过明细：{error}")
        })?;
    }
    tx.commit()
        .map_err(|error| format!("{SCAN_STOPPED_MESSAGE}；无法保存已产生的跳过明细：{error}"))
}

fn record_scan_skip(conn: &Connection, scan_id: i64, skip: &PendingSkip) -> Result<(), String> {
    let metadata = fs::metadata(&skip.path).ok();
    let file_name = skip
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| skip.path.to_string_lossy().to_string());
    let path_string = skip.path.to_string_lossy().to_string();
    let root_string = skip.root_path.to_string_lossy().to_string();
    let file_size = metadata.as_ref().map(|metadata| metadata.len() as i64);
    let modified_ms = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);

    conn.execute(
        r#"
        INSERT INTO scan_skips (
          scan_id, path, file_name, root_path, root_key, reason, detail,
          is_short_video, file_size, modified_ms
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        "#,
        params![
            scan_id,
            path_string,
            file_name,
            root_string,
            &skip.root_key,
            &skip.reason,
            &skip.detail,
            if skip.is_short_video { 1 } else { 0 },
            file_size,
            modified_ms
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn load_scan_skips(
    conn: &Connection,
    scan_id: i64,
    include_short_video: bool,
) -> Result<Vec<ScanSkip>, String> {
    let sql = if include_short_video {
        String::from(
            r#"
            SELECT id, scan_id, path, file_name, root_path, reason, detail, is_short_video,
                   file_size, modified_ms, created_at
            FROM scan_skips
            WHERE scan_id = ?1
            ORDER BY root_path COLLATE NOCASE, path COLLATE NOCASE
            "#,
        )
    } else {
        String::from(
            r#"
        SELECT id, scan_id, path, file_name, root_path, reason, detail, is_short_video,
               file_size, modified_ms, created_at
        FROM scan_skips
        WHERE scan_id = ?1 AND is_short_video = 0
        ORDER BY root_path COLLATE NOCASE, path COLLATE NOCASE
        "#,
        )
    };

    let mut stmt = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![scan_id], |row| {
            Ok(ScanSkip {
                id: row.get(0)?,
                scan_id: row.get(1)?,
                path: row.get(2)?,
                file_name: row.get(3)?,
                root_path: row.get(4)?,
                reason: row.get(5)?,
                detail: row.get(6)?,
                is_short_video: row.get::<_, i64>(7)? != 0,
                file_size: row.get(8)?,
                modified_ms: row.get(9)?,
                created_at: row.get(10)?,
            })
        })
        .map_err(|error| error.to_string())?;

    let mut skips = Vec::new();
    for row in rows {
        skips.push(row.map_err(|error| error.to_string())?);
    }
    Ok(skips)
}

fn count_recorded_directories(conn: &Connection) -> Result<usize, String> {
    conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM (
          SELECT DISTINCT media_kind, directory_path
          FROM media_files
          WHERE media_kind IN ('music', 'video') AND directory_path <> ''
        )
        "#,
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count as usize)
    .map_err(|error| error.to_string())
}

fn create_scan_run(
    conn: &Connection,
    started_at_ms: i64,
    paths: &[String],
    excluded_paths: &[String],
) -> Result<i64, String> {
    let paths_json =
        serde_json::to_string(paths).map_err(|error| format!("扫描目录序列化失败：{error}"))?;
    let excluded_paths_json = serde_json::to_string(excluded_paths)
        .map_err(|error| format!("排除目录序列化失败：{error}"))?;
    conn.execute(
        r#"
        INSERT INTO scan_runs (
          started_at_ms, completed_at_ms, duration_ms, scanned_files, imported_files,
          skipped_files, skipped_short_files, recorded_directories, ffprobe_missing,
          status, paths_json, excluded_paths_json, failed_roots_json
        )
        VALUES (?1, 0, 0, 0, 0, 0, 0, 0, 0, 'running', ?2, ?3, '[]')
        "#,
        params![started_at_ms, paths_json, excluded_paths_json],
    )
    .map_err(|error| format!("创建扫描历史失败：{error}"))?;
    Ok(conn.last_insert_rowid())
}

fn finish_scan_run(
    conn: &Connection,
    summary: &ScanSummary,
    error_message: Option<&str>,
) -> Result<(), String> {
    let failed_roots_json = serde_json::to_string(&summary.failed_roots)
        .map_err(|error| format!("失败目录序列化失败：{error}"))?;
    let changed = conn
        .execute(
            r#"
            UPDATE scan_runs
            SET completed_at_ms = ?2,
                duration_ms = ?3,
                scanned_files = ?4,
                imported_files = ?5,
                skipped_files = ?6,
                skipped_short_files = ?7,
                recorded_directories = ?8,
                ffprobe_missing = ?9,
                status = ?10,
                error_message = ?11,
                failed_roots_json = ?12
            WHERE id = ?1
            "#,
            params![
                summary.scan_id,
                summary.completed_at_ms,
                summary.duration_ms,
                summary.scanned_files as i64,
                summary.imported_files as i64,
                summary.skipped_files as i64,
                summary.skipped_short_files as i64,
                summary.recorded_directories as i64,
                if summary.ffprobe_missing { 1 } else { 0 },
                &summary.status,
                error_message,
                failed_roots_json
            ],
        )
        .map_err(|error| format!("更新扫描历史失败：{error}"))?;
    if changed != 1 {
        return Err(format!("扫描历史 {} 不存在", summary.scan_id));
    }
    Ok(())
}

fn parse_string_list_json(value: &str, context: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<Vec<String>>(value)
        .map_err(|error| format!("{context}格式损坏：{error}"))
}

fn parse_failed_roots_json(value: &str, context: &str) -> Result<Vec<FailedRoot>, String> {
    serde_json::from_str::<Vec<FailedRoot>>(value)
        .map_err(|error| format!("{context}格式损坏：{error}"))
}

fn load_scan_history(conn: &Connection, limit: i64) -> Result<Vec<ScanRun>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, started_at_ms, completed_at_ms, duration_ms, scanned_files,
                   imported_files, skipped_files, skipped_short_files, recorded_directories,
                   ffprobe_missing, status, error_message, failed_roots_json,
                   paths_json, excluded_paths_json, created_at
            FROM scan_runs
            ORDER BY id DESC
            LIMIT ?1
            "#,
        )
        .map_err(|error| error.to_string())?;
    let mut rows = stmt
        .query(params![limit])
        .map_err(|error| error.to_string())?;
    let mut runs = Vec::new();

    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let id: i64 = row.get(0).map_err(|error| error.to_string())?;
        let paths_json: String = row.get(13).map_err(|error| error.to_string())?;
        let excluded_paths_json: String = row.get(14).map_err(|error| error.to_string())?;
        let failed_roots_json: String = row.get(12).map_err(|error| error.to_string())?;
        runs.push(ScanRun {
            id,
            started_at_ms: row.get(1).map_err(|error| error.to_string())?,
            completed_at_ms: row.get(2).map_err(|error| error.to_string())?,
            duration_ms: row.get(3).map_err(|error| error.to_string())?,
            scanned_files: row.get(4).map_err(|error| error.to_string())?,
            imported_files: row.get(5).map_err(|error| error.to_string())?,
            skipped_files: row.get(6).map_err(|error| error.to_string())?,
            skipped_short_files: row.get(7).map_err(|error| error.to_string())?,
            recorded_directories: row.get(8).map_err(|error| error.to_string())?,
            ffprobe_missing: row.get::<_, i64>(9).map_err(|error| error.to_string())? != 0,
            status: row.get(10).map_err(|error| error.to_string())?,
            error_message: row.get(11).map_err(|error| error.to_string())?,
            failed_roots: parse_failed_roots_json(
                &failed_roots_json,
                &format!("扫描历史 {id} 的失败目录"),
            )?,
            paths: parse_string_list_json(&paths_json, &format!("扫描历史 {id} 的扫描目录"))?,
            excluded_paths: parse_string_list_json(
                &excluded_paths_json,
                &format!("扫描历史 {id} 的排除目录"),
            )?,
            created_at: row.get(15).map_err(|error| error.to_string())?,
        });
    }
    Ok(runs)
}

fn load_last_scan_config(conn: &Connection) -> Result<ScanConfig, String> {
    let row = conn
        .query_row(
            r#"
            SELECT id, paths_json, excluded_paths_json
            FROM scan_runs
            ORDER BY id DESC
            LIMIT 1
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;

    match row {
        Some((id, paths_json, excluded_paths_json)) => Ok(ScanConfig {
            paths: parse_string_list_json(&paths_json, &format!("扫描历史 {id} 的扫描目录"))?,
            excluded_paths: parse_string_list_json(
                &excluded_paths_json,
                &format!("扫描历史 {id} 的排除目录"),
            )?,
        }),
        None => Ok(ScanConfig {
            paths: Vec::new(),
            excluded_paths: Vec::new(),
        }),
    }
}

#[derive(Debug)]
struct MediaGroupBuilder {
    key: String,
    name: String,
    subtitle: Option<String>,
    family_name: Option<String>,
    file_count: usize,
    total_size: i64,
    source_keys: HashSet<String>,
    resource_keys: HashSet<String>,
    child_groups: HashMap<String, MediaGroupBuilder>,
}

impl MediaGroupBuilder {
    fn new(
        key: String,
        name: String,
        subtitle: Option<String>,
        family_name: Option<String>,
    ) -> Self {
        Self {
            key,
            name,
            subtitle,
            family_name,
            file_count: 0,
            total_size: 0,
            source_keys: HashSet::new(),
            resource_keys: HashSet::new(),
            child_groups: HashMap::new(),
        }
    }

    fn add_file(
        &mut self,
        file_size: i64,
        source_key: String,
        resource_key: String,
        subtitle: Option<String>,
    ) {
        if self.file_count > 0 && self.subtitle != subtitle {
            self.subtitle = None;
        }
        self.file_count += 1;
        self.total_size += file_size;
        self.source_keys.insert(source_key);
        self.resource_keys.insert(resource_key);
    }

    fn finish(self) -> MediaGroup {
        let mut source_keys: Vec<String> = self.source_keys.into_iter().collect();
        let mut resource_keys: Vec<String> = self.resource_keys.into_iter().collect();
        source_keys.sort();
        resource_keys.sort();
        let mut child_groups: Vec<MediaGroup> = self
            .child_groups
            .into_values()
            .map(MediaGroupBuilder::finish)
            .collect();
        sort_groups(&mut child_groups);
        MediaGroup {
            key: self.key,
            name: self.name,
            subtitle: self.subtitle,
            family_name: self.family_name,
            file_count: self.file_count,
            total_size: self.total_size,
            source_keys,
            resource_keys,
            child_groups,
        }
    }
}

fn load_library(conn: &Connection, query: Option<&str>) -> Result<LibraryData, String> {
    let music_artist_rules = load_merge_rules(conn, "music_artist")?;
    let video_series_rules = load_merge_rules(conn, "video_series")?;
    let video_family_rules = load_merge_rules(conn, "video_family")?;
    let search_text = query
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let mut music_directories: HashMap<String, MediaDirectory> = HashMap::new();
    let mut video_directories: HashMap<String, MediaDirectory> = HashMap::new();
    let mut music_artists: HashMap<String, MediaGroupBuilder> = HashMap::new();
    let mut video_series: HashMap<String, MediaGroupBuilder> = HashMap::new();
    let mut matched_directories: HashSet<String> = HashSet::new();
    let mut matched_groups: HashSet<String> = HashSet::new();

    let mut stmt = conn
        .prepare(
            r#"
            SELECT
              id, path, file_name, root_path, file_size, duration_seconds, container,
              video_codec, audio_codec, width, height, resolution, source, release_group,
              season_number, episode_number, title_guess, media_kind, music_artist, music_album,
              music_title, music_artist_source, series_title, series_source
            FROM media_files
            WHERE media_kind IN ('music', 'video')
            "#,
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], resource_variant_from_row)
        .map_err(|error| error.to_string())?;

    for row in rows {
        let file = row.map_err(|error| error.to_string())?;
        let directory_key = add_file_to_directory(
            if file.media_kind == "music" {
                &mut music_directories
            } else {
                &mut video_directories
            },
            &file,
        );

        let file_matches = search_text.as_ref().is_some_and(|search| {
            file.file_name.to_lowercase().contains(search)
                || file.path.to_lowercase().contains(search)
        });
        if file_matches {
            matched_directories.insert(media_directory_match_key(&file.media_kind, &directory_key));
        }

        if file.media_kind == "music" {
            let artist = detected_artist_for_file(&file);
            let source_key = group_source_key("music_artist", &artist);
            let display_name = apply_merge_rule(&source_key, artist, &music_artist_rules);
            let group_key = group_source_key("music_artist", &display_name);
            let subtitle = file
                .music_artist_source
                .as_deref()
                .map(music_artist_source_label);
            let group = music_artists.entry(group_key.clone()).or_insert_with(|| {
                MediaGroupBuilder::new(group_key.clone(), display_name, subtitle.clone(), None)
            });
            group.add_file(file.file_size, source_key.clone(), source_key, subtitle);
            if file_matches {
                matched_groups.insert(group_key);
            }
            continue;
        }

        let detected_series =
            detected_series_for_file(&file).unwrap_or_else(|| "未识别系列".to_string());
        let source_key = group_source_key("video_series", &detected_series);
        let display_name =
            apply_merge_rule(&source_key, detected_series.clone(), &video_series_rules);
        let subtitle = file.series_source.as_deref().map(series_source_label);
        let family = video_family_rules
            .get(&source_key)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .or_else(|| {
                infer_series_family(
                    Path::new(&file.path),
                    Path::new(&file.root_path),
                    &detected_series,
                )
            });

        if let Some(family_name) = family {
            let parent_source_key = group_source_key("video_series", &family_name);
            let parent_name =
                apply_merge_rule(&parent_source_key, family_name, &video_series_rules);
            let parent_key = group_source_key("video_series", &parent_name);
            let parent = video_series.entry(parent_key.clone()).or_insert_with(|| {
                MediaGroupBuilder::new(
                    parent_key.clone(),
                    parent_name.clone(),
                    Some("作品族".to_string()),
                    None,
                )
            });
            parent.add_file(
                file.file_size,
                parent_source_key,
                source_key.clone(),
                Some("作品族".to_string()),
            );

            let child_key = group_source_key("video_series", &display_name);
            let child = parent
                .child_groups
                .entry(child_key.clone())
                .or_insert_with(|| {
                    MediaGroupBuilder::new(
                        child_key.clone(),
                        display_name,
                        subtitle.clone(),
                        Some(parent_name),
                    )
                });
            child.add_file(file.file_size, source_key.clone(), source_key, subtitle);
            if file_matches {
                matched_groups.insert(parent_key);
                matched_groups.insert(child_key);
            }
        } else {
            let group_key = group_source_key("video_series", &display_name);
            let group = video_series.entry(group_key.clone()).or_insert_with(|| {
                MediaGroupBuilder::new(group_key.clone(), display_name, subtitle.clone(), None)
            });
            group.add_file(file.file_size, source_key.clone(), source_key, subtitle);
            if file_matches {
                matched_groups.insert(group_key);
            }
        }
    }

    let mut music_directories: Vec<MediaDirectory> = music_directories.into_values().collect();
    let mut video_directories: Vec<MediaDirectory> = video_directories.into_values().collect();
    let mut music_artists: Vec<MediaGroup> = music_artists
        .into_values()
        .map(MediaGroupBuilder::finish)
        .collect();
    let mut video_series: Vec<MediaGroup> = video_series
        .into_values()
        .map(MediaGroupBuilder::finish)
        .collect();

    if let Some(search) = search_text.as_deref() {
        music_directories.retain(|directory| {
            directory_summary_matches(directory, search)
                || matched_directories.contains(&media_directory_match_key("music", &directory.key))
        });
        video_directories.retain(|directory| {
            directory_summary_matches(directory, search)
                || matched_directories.contains(&media_directory_match_key("video", &directory.key))
        });
        retain_matching_groups(&mut music_artists, search, &matched_groups);
        retain_matching_groups(&mut video_series, search, &matched_groups);
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

fn media_directory_match_key(media_kind: &str, directory_key: &str) -> String {
    format!("{media_kind}:{directory_key}")
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

fn apply_merge_rule(
    source_key: &str,
    detected_name: String,
    rules: &HashMap<String, String>,
) -> String {
    rules
        .get(source_key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or(detected_name)
}

fn detected_artist_for_file(file: &ResourceVariant) -> String {
    file.music_artist
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| infer_artist_from_directory(Path::new(&file.path), Path::new(&file.root_path)))
        .unwrap_or_else(|| "未知作者".to_string())
}

fn detected_series_for_file(file: &ResourceVariant) -> Option<String> {
    let stored_series = file
        .series_title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_series_title(&value));
    if let Some(stored) = stored_series {
        if is_generated_directory_title(&stored) {
            if let Some(filename) = infer_series_from_file_name(&file.file_name) {
                if should_replace_stored_series(&stored, &filename) {
                    return Some(filename);
                }
            }
        }
        return Some(stored);
    }

    infer_series_from_file_name(&file.file_name).or_else(|| {
        if is_detected_title(&file.title_guess) {
            Some(normalize_series_title(&file.title_guess))
        } else {
            infer_series_from_directory(Path::new(&file.path), Path::new(&file.root_path))
        }
    })
}

fn add_file_to_directory(
    directories: &mut HashMap<String, MediaDirectory>,
    file: &ResourceVariant,
) -> String {
    let directory_path = Path::new(&file.path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| file.root_path.clone());
    let key = directory_path.clone();
    let directory = directories.entry(key.clone()).or_insert_with(|| {
        let name = Path::new(&directory_path)
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "根目录".to_string());
        let parent_name = Path::new(&directory_path)
            .parent()
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().to_string());
        MediaDirectory {
            key: key.clone(),
            path: directory_path.clone(),
            name,
            relative_path: relative_directory_path(&file.root_path, &directory_path),
            parent_name,
            media_kind: file.media_kind.clone(),
            file_count: 0,
            total_size: 0,
        }
    });
    directory.file_count += 1;
    directory.total_size += file.file_size;
    key
}

fn directory_summary_matches(directory: &MediaDirectory, search: &str) -> bool {
    directory.name.to_lowercase().contains(search)
        || directory.relative_path.to_lowercase().contains(search)
        || directory.path.to_lowercase().contains(search)
}

fn retain_matching_groups(
    groups: &mut Vec<MediaGroup>,
    search: &str,
    matched_groups: &HashSet<String>,
) {
    for group in groups.iter_mut() {
        retain_matching_groups(&mut group.child_groups, search, matched_groups);
    }
    groups.retain(|group| {
        group.name.to_lowercase().contains(search)
            || group
                .subtitle
                .as_deref()
                .is_some_and(|subtitle| subtitle.to_lowercase().contains(search))
            || group
                .source_keys
                .iter()
                .any(|key| key.to_lowercase().contains(search))
            || matched_groups.contains(&group.key)
            || !group.child_groups.is_empty()
    });
}

fn load_resource_page(conn: &Connection, request: ResourceQuery) -> Result<ResourcePage, String> {
    if !matches!(request.media_kind.as_str(), "music" | "video") {
        return Err("不支持的媒体类型".to_string());
    }
    if (request.kind == "music_artist" && request.media_kind != "music")
        || (request.kind == "video_series" && request.media_kind != "video")
    {
        return Err("资源分类与媒体类型不匹配".to_string());
    }
    let limit = if request.limit == 0 {
        RESOURCE_PAGE_DEFAULT_LIMIT
    } else {
        request.limit.min(RESOURCE_PAGE_MAX_LIMIT)
    };
    let (where_clause, values) = match request.kind.as_str() {
        "directory" => {
            if request.key.trim().is_empty() {
                return Err("目录查询缺少目录键".to_string());
            }
            (
                "media_kind = ? AND directory_path = ?".to_string(),
                vec![Value::Text(request.media_kind), Value::Text(request.key)],
            )
        }
        "music_artist" | "video_series" => {
            if request.source_keys.is_empty() || request.source_keys.len() > 400 {
                return Err("分类资源键数量必须在 1 到 400 之间".to_string());
            }
            let expected_prefix = format!("{}:", request.kind);
            if request
                .source_keys
                .iter()
                .any(|key| key.len() > 512 || !key.starts_with(&expected_prefix))
            {
                return Err("分类资源键格式无效".to_string());
            }
            let column = if request.kind == "music_artist" {
                "music_artist_key"
            } else {
                "series_key"
            };
            let placeholders = vec!["?"; request.source_keys.len()].join(", ");
            let mut values = vec![Value::Text(request.media_kind)];
            values.extend(request.source_keys.into_iter().map(Value::Text));
            (
                format!("media_kind = ? AND {column} IN ({placeholders})"),
                values,
            )
        }
        _ => return Err("不支持的资源查询类型".to_string()),
    };

    let count_sql = format!("SELECT COUNT(*) FROM media_files WHERE {where_clause}");
    let total = conn
        .query_row(&count_sql, params_from_iter(values.iter()), |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())? as usize;

    let select_sql = format!(
        r#"
        SELECT
          id, path, file_name, root_path, file_size, duration_seconds, container,
          video_codec, audio_codec, width, height, resolution, source, release_group,
          season_number, episode_number, title_guess, media_kind, music_artist, music_album,
          music_title, music_artist_source, series_title, series_source
        FROM media_files
        WHERE {where_clause}
        ORDER BY
          CASE WHEN season_number IS NULL THEN 1 ELSE 0 END,
          season_number,
          CASE WHEN episode_number IS NULL THEN 1 ELSE 0 END,
          episode_number,
          file_name COLLATE NOCASE,
          path COLLATE NOCASE
        LIMIT ? OFFSET ?
        "#
    );
    let mut page_values = values;
    page_values.push(Value::Integer(limit as i64));
    page_values.push(Value::Integer(request.offset as i64));
    let mut stmt = conn
        .prepare(&select_sql)
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(
            params_from_iter(page_values.iter()),
            resource_variant_from_row,
        )
        .map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for row in rows {
        files.push(row.map_err(|error| error.to_string())?);
    }

    Ok(ResourcePage {
        files,
        total,
        offset: request.offset,
        limit,
    })
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
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "根目录".to_string())
}

fn sort_directories(directories: &mut [MediaDirectory]) {
    directories.sort_by_cached_key(|directory| directory.relative_path.to_lowercase());
}

fn sort_groups(groups: &mut [MediaGroup]) {
    groups.sort_by_cached_key(|group| group.name.to_lowercase());
    for group in groups {
        sort_groups(&mut group.child_groups);
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
          root_key TEXT NOT NULL DEFAULT '',
          directory_path TEXT NOT NULL DEFAULT '',
          media_kind TEXT NOT NULL DEFAULT '',
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
          music_artist_key TEXT,
          series_title TEXT,
          series_source TEXT,
          series_key TEXT,
          family_key TEXT,
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

        CREATE TABLE IF NOT EXISTS scan_runs (
          id INTEGER PRIMARY KEY,
          started_at_ms INTEGER NOT NULL,
          completed_at_ms INTEGER NOT NULL,
          duration_ms INTEGER NOT NULL,
          scanned_files INTEGER NOT NULL,
          imported_files INTEGER NOT NULL,
          skipped_files INTEGER NOT NULL,
          skipped_short_files INTEGER NOT NULL,
          recorded_directories INTEGER NOT NULL,
          ffprobe_missing INTEGER NOT NULL DEFAULT 0,
          status TEXT NOT NULL DEFAULT 'completed',
          error_message TEXT,
          failed_roots_json TEXT NOT NULL DEFAULT '[]',
          paths_json TEXT NOT NULL DEFAULT '[]',
          excluded_paths_json TEXT NOT NULL DEFAULT '[]',
          created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS scan_skips (
          id INTEGER PRIMARY KEY,
          scan_id INTEGER,
          path TEXT NOT NULL,
          file_name TEXT NOT NULL,
          root_path TEXT NOT NULL,
          root_key TEXT NOT NULL DEFAULT '',
          reason TEXT NOT NULL,
          detail TEXT NOT NULL,
          is_short_video INTEGER NOT NULL DEFAULT 0,
          file_size INTEGER,
          modified_ms INTEGER,
          created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
          updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )?;

    add_column_if_missing(conn, "media_files", "music_artist", "TEXT")?;
    add_column_if_missing(conn, "media_files", "music_album", "TEXT")?;
    add_column_if_missing(conn, "media_files", "music_title", "TEXT")?;
    add_column_if_missing(conn, "media_files", "music_artist_source", "TEXT")?;
    add_column_if_missing(conn, "media_files", "series_title", "TEXT")?;
    add_column_if_missing(conn, "media_files", "series_source", "TEXT")?;
    add_column_if_missing(conn, "media_files", "root_key", "TEXT NOT NULL DEFAULT ''")?;
    add_column_if_missing(
        conn,
        "media_files",
        "directory_path",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "media_files",
        "media_kind",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(conn, "media_files", "music_artist_key", "TEXT")?;
    add_column_if_missing(conn, "media_files", "series_key", "TEXT")?;
    add_column_if_missing(conn, "media_files", "family_key", "TEXT")?;
    add_column_if_missing(
        conn,
        "scan_runs",
        "status",
        "TEXT NOT NULL DEFAULT 'completed'",
    )?;
    add_column_if_missing(conn, "scan_runs", "error_message", "TEXT")?;
    add_column_if_missing(
        conn,
        "scan_runs",
        "failed_roots_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;

    migrate_scan_skips(conn)?;

    let schema_version =
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if schema_version < DATABASE_SCHEMA_VERSION {
        let tx = conn.unchecked_transaction()?;
        backfill_media_index_fields(&tx)?;
        backfill_scan_skip_root_keys(&tx)?;
        tx.pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)?;
        tx.commit()?;
    }

    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_media_files_item_key ON media_files(item_key);
        CREATE INDEX IF NOT EXISTS idx_media_files_title ON media_files(title_guess);
        CREATE INDEX IF NOT EXISTS idx_media_files_root_key ON media_files(root_key);
        CREATE INDEX IF NOT EXISTS idx_media_files_directory ON media_files(media_kind, directory_path);
        CREATE INDEX IF NOT EXISTS idx_media_files_artist_key ON media_files(music_artist_key);
        CREATE INDEX IF NOT EXISTS idx_media_files_series_key ON media_files(series_key);
        CREATE INDEX IF NOT EXISTS idx_media_files_family_key ON media_files(family_key);
        CREATE INDEX IF NOT EXISTS idx_merge_rules_kind ON merge_rules(kind);
        CREATE INDEX IF NOT EXISTS idx_scan_skips_scan ON scan_skips(scan_id, is_short_video);
        CREATE INDEX IF NOT EXISTS idx_scan_skips_root_key ON scan_skips(root_key);
        CREATE INDEX IF NOT EXISTS idx_scan_runs_completed ON scan_runs(completed_at_ms DESC);
        "#,
    )?;
    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing?.eq_ignore_ascii_case(column) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    if table_has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn migrate_scan_skips(conn: &Connection) -> Result<(), rusqlite::Error> {
    if table_has_column(conn, "scan_skips", "scan_id")? {
        return Ok(());
    }

    // 旧表按路径覆盖记录；迁移后保留这些记录并标记为未关联历史。
    conn.execute_batch(
        r#"
        BEGIN IMMEDIATE;
        ALTER TABLE scan_skips RENAME TO scan_skips_legacy;
        CREATE TABLE scan_skips (
          id INTEGER PRIMARY KEY,
          scan_id INTEGER,
          path TEXT NOT NULL,
          file_name TEXT NOT NULL,
          root_path TEXT NOT NULL,
          root_key TEXT NOT NULL DEFAULT '',
          reason TEXT NOT NULL,
          detail TEXT NOT NULL,
          is_short_video INTEGER NOT NULL DEFAULT 0,
          file_size INTEGER,
          modified_ms INTEGER,
          created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
          updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO scan_skips (
          id, scan_id, path, file_name, root_path, root_key, reason, detail,
          is_short_video, file_size, modified_ms, created_at, updated_at
        )
        SELECT
          id, NULL, path, file_name, root_path, '', reason, detail,
          is_short_video, file_size, modified_ms, created_at, updated_at
        FROM scan_skips_legacy;
        DROP TABLE scan_skips_legacy;
        COMMIT;
        "#,
    )
}

fn resource_variant_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceVariant> {
    Ok(ResourceVariant {
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
        media_kind: row.get(17)?,
        music_artist: row.get(18)?,
        music_album: row.get(19)?,
        music_title: row.get(20)?,
        music_artist_source: row.get(21)?,
        series_title: row.get(22)?,
        series_source: row.get(23)?,
    })
}

fn backfill_media_index_fields(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
          id, path, file_name, root_path, file_size, duration_seconds, container,
          video_codec, audio_codec, width, height, resolution, source, release_group,
          season_number, episode_number, title_guess, media_kind, music_artist, music_album,
          music_title, music_artist_source, series_title, series_source
        FROM media_files
        WHERE root_key = '' OR directory_path = '' OR media_kind = ''
           OR (media_kind = 'music' AND music_artist_key IS NULL)
           OR (media_kind = 'video' AND series_key IS NULL)
        "#,
    )?;
    let rows = stmt.query_map([], resource_variant_from_row)?;
    let mut updates = Vec::new();
    for row in rows {
        let mut file = row?;
        if file.media_kind.is_empty() {
            file.media_kind = media_kind_for_file(&file).unwrap_or_else(|| "ignored".to_string());
        }
        let directory_path = Path::new(&file.path)
            .parent()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| file.root_path.clone());
        let artist = file
            .music_artist
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                infer_artist_from_directory(Path::new(&file.path), Path::new(&file.root_path))
            });
        let series = detected_series_for_file(&file);
        let family = series.as_deref().and_then(|series| {
            infer_series_family(Path::new(&file.path), Path::new(&file.root_path), series)
        });
        updates.push((
            file.id,
            normalized_path_string(Path::new(&file.root_path)),
            directory_path,
            file.media_kind,
            artist.map(|value| group_source_key("music_artist", &value)),
            series.map(|value| group_source_key("video_series", &value)),
            family.map(|value| group_source_key("video_series", &value)),
        ));
    }
    drop(stmt);

    for (id, root_key, directory_path, media_kind, artist_key, series_key, family_key) in updates {
        conn.execute(
            r#"
            UPDATE media_files
            SET root_key = ?2,
                directory_path = ?3,
                media_kind = ?4,
                music_artist_key = ?5,
                series_key = ?6,
                family_key = ?7
            WHERE id = ?1
            "#,
            params![
                id,
                root_key,
                directory_path,
                media_kind,
                artist_key,
                series_key,
                family_key
            ],
        )?;
    }
    Ok(())
}

fn backfill_scan_skip_root_keys(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT id, root_path FROM scan_skips WHERE root_key = ''")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut updates = Vec::new();
    for row in rows {
        let (id, root_path) = row?;
        updates.push((id, normalized_path_string(Path::new(&root_path))));
    }
    drop(stmt);

    for (id, root_key) in updates {
        conn.execute(
            "UPDATE scan_skips SET root_key = ?2 WHERE id = ?1",
            params![id, root_key],
        )?;
    }
    Ok(())
}

fn open_connection(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|error| format!("无法打开数据库：{error}"))?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|error| format!("无法设置数据库等待时间：{error}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| format!("无法启用数据库外键：{error}"))?;
    Ok(conn)
}

fn setup_database(app: &tauri::AppHandle) -> Result<DbState, Box<dyn std::error::Error>> {
    let db_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("media_administrator.sqlite3");

    if !db_path.exists() {
        let app_data_dir = app.path().app_data_dir()?;
        let old_db_path = app_data_dir.join("media_administrator.sqlite3");
        if old_db_path.exists() {
            let temporary_path = db_path.with_extension("sqlite3.migrating");
            if temporary_path.exists() {
                fs::remove_file(&temporary_path)?;
            }
            let source = Connection::open(&old_db_path)?;
            let mut destination = Connection::open(&temporary_path)?;
            {
                // SQLite 在线备份会一并读取 WAL 中已提交的数据，避免直接复制主文件造成丢失。
                let backup = rusqlite::backup::Backup::new(&source, &mut destination)?;
                backup.run_to_completion(128, Duration::from_millis(10), None)?;
            }
            let integrity: String =
                destination.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            drop(destination);
            drop(source);
            if integrity != "ok" {
                fs::remove_file(&temporary_path)?;
                return Err(format!("旧数据库完整性检查失败：{integrity}").into());
            }
            fs::rename(temporary_path, &db_path)?;
        }
    }

    let conn = open_connection(&db_path).map_err(std::io::Error::other)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    init_schema(&conn)?;
    Ok(DbState {
        db_path,
        scan_running: Mutex::new(false),
        stop_requested: Arc::new(AtomicBool::new(false)),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished {
                let window = webview.window();
                let _ = window.show();
                let _ = window.set_focus();
            }
        })
        .setup(|app| {
            let db = setup_database(app.handle())?;
            app.manage(db);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_ffprobe,
            start_scan,
            list_library,
            list_resources,
            list_scan_skips,
            list_scan_history,
            get_last_scan_config,
            set_merge_rules,
            stop_scan
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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

    fn insert_video(conn: &Connection, path: &str, root: &str, series: &str) {
        let root_key = normalized_path_string(Path::new(root));
        let directory = Path::new(path)
            .parent()
            .expect("test video has parent")
            .to_string_lossy()
            .to_string();
        let series_key = group_source_key("video_series", series);
        conn.execute(
            r#"
            INSERT INTO media_files (
              path, file_name, root_path, root_key, directory_path, media_kind,
              file_size, video_codec, title_guess, item_key, series_title,
              series_source, series_key
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'video', 10, 'H.264', ?6, ?7, ?6, 'filename', ?8)
            "#,
            params![
                path,
                Path::new(path)
                    .file_name()
                    .expect("test video has name")
                    .to_string_lossy(),
                root,
                root_key,
                directory,
                series,
                build_item_key(series, Some(1), Some(1)),
                series_key
            ],
        )
        .expect("insert test video");
    }

    fn insert_music(conn: &Connection, path: &str, root: &str, artist: &str) {
        let root_key = normalized_path_string(Path::new(root));
        let directory = Path::new(path)
            .parent()
            .expect("test music has parent")
            .to_string_lossy()
            .to_string();
        let artist_key = group_source_key("music_artist", artist);
        conn.execute(
            r#"
            INSERT INTO media_files (
              path, file_name, root_path, root_key, directory_path, media_kind,
              file_size, audio_codec, title_guess, item_key, music_artist,
              music_artist_source, music_artist_key
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'music', 10, 'FLAC', ?6, ?7, ?8, 'tag', ?9)
            "#,
            params![
                path,
                Path::new(path)
                    .file_name()
                    .expect("test music has name")
                    .to_string_lossy(),
                root,
                root_key,
                directory,
                path,
                build_item_key(path, None, None),
                artist,
                artist_key
            ],
        )
        .expect("insert test music");
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
    fn bracketed_filename_series_replaces_date_directory_title() {
        let mut file = resource_variant("[Nekomoe kissaten][Jigokuraku][21][1080p][JPSC].mp4");
        file.video_codec = Some("H.264".to_string());
        file.series_title = Some("202604".to_string());
        file.title_guess = "202604".to_string();

        assert_eq!(
            detected_series_for_file(&file).as_deref(),
            Some("Jigokuraku")
        );
    }

    #[test]
    fn excluded_directory_skips_entire_subtree_only() {
        let excluded = build_excluded_roots(&["/library/root/extras".to_string()]);

        assert!(is_excluded_path(
            Path::new("/library/root/extras"),
            &excluded
        ));
        assert!(is_excluded_path(
            Path::new("/library/root/extras/bonus/file.mkv"),
            &excluded
        ));
        assert!(!is_excluded_path(
            Path::new("/library/root/extras2/file.mkv"),
            &excluded
        ));
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
    fn parses_seed_raws_monogatari_episode_name() {
        let parsed = parse_name("[Seed-Raws] Bakemonogatari - 01 (BD 1280x720 AVC AACx2).mp4");

        assert_eq!(parsed.release_group.as_deref(), Some("Seed-Raws"));
        assert_eq!(parsed.title_guess, "Bakemonogatari");
        assert_eq!(parsed.season_number, Some(1));
        assert_eq!(parsed.episode_number, Some(1));
        assert_eq!(
            infer_series_family_title(&parsed.title_guess).as_deref(),
            Some("Monogatari")
        );
    }

    #[test]
    fn leading_number_episode_uses_directory_series() {
        let parsed = parse_name("01 - Awakening [BDRip 1080p].mkv");

        assert_eq!(parsed.title_guess, "未命名资源");
        assert_eq!(parsed.season_number, Some(1));
        assert_eq!(parsed.episode_number, Some(1));
        assert_eq!(
            infer_video_series(
                Path::new("/library/Some Anime/01 - Awakening [BDRip 1080p].mkv"),
                Path::new("/library"),
                &parsed
            ),
            (
                Some("Some Anime".to_string()),
                Some("directory".to_string())
            )
        );
    }

    #[test]
    fn technical_only_number_episode_uses_directory_series() {
        let parsed = parse_name("10 [1080p][BDRip].mkv");

        assert_eq!(parsed.title_guess, "未命名资源");
        assert_eq!(parsed.season_number, Some(1));
        assert_eq!(parsed.episode_number, Some(10));
    }

    #[test]
    fn numeric_movie_title_stays_filename_series() {
        let parsed = parse_name("12 Angry Men.mkv");

        assert_eq!(parsed.title_guess, "12 Angry Men");
        assert_eq!(parsed.season_number, None);
        assert_eq!(parsed.episode_number, None);
    }

    #[test]
    fn normalizes_case_and_strips_video_technical_suffix() {
        assert_eq!(normalize_series_title("kara no kyoukai"), "Kara no Kyoukai");
        assert_eq!(
            normalize_series_title("Kara no Kyoukai - BD-BOX Disc8 Remix (BD 12"),
            "Kara no Kyoukai"
        );
    }

    #[test]
    fn recognizes_monogatari_title_family() {
        let mut file = resource_variant("Bakemonogatari - 01.mkv");
        file.file_size = 10;
        file.video_codec = Some("H.264".to_string());
        file.series_title = Some("Bakemonogatari".to_string());
        file.series_source = Some("filename".to_string());

        assert_eq!(
            infer_series_family(
                Path::new("/library/Bakemonogatari/Bakemonogatari - 01.mkv"),
                Path::new("/library"),
                file.series_title.as_deref().expect("series")
            )
            .as_deref(),
            Some("Monogatari")
        );
    }

    #[test]
    fn parses_artist_title_from_music_filename() {
        let parsed = infer_artist_title_from_filename("artist name - song title.flac");

        assert_eq!(
            parsed,
            Some(("artist name".to_string(), "song title".to_string()))
        );
    }

    #[test]
    fn explicit_music_filename_artist_precedes_directory() {
        let probe = MediaProbe {
            has_audio: true,
            ..MediaProbe::default()
        };
        let metadata = infer_music_metadata(
            &probe,
            Path::new("/OST/Directory Artist/Album/File Artist - Song.flac"),
            Path::new("/OST"),
            "File Artist - Song.flac",
        );

        assert_eq!(metadata.0.as_deref(), Some("File Artist"));
        assert_eq!(metadata.3.as_deref(), Some("filename"));
    }

    #[test]
    fn numeric_track_prefix_uses_directory_artist() {
        let probe = MediaProbe {
            has_audio: true,
            ..MediaProbe::default()
        };
        let metadata = infer_music_metadata(
            &probe,
            Path::new("/OST/Directory Artist/Album/01 - Song.flac"),
            Path::new("/OST"),
            "01 - Song.flac",
        );

        assert_eq!(metadata.0.as_deref(), Some("Directory Artist"));
        assert_eq!(metadata.3.as_deref(), Some("directory"));
    }

    #[test]
    fn music_directory_metadata_skips_disc_folders() {
        let probe = MediaProbe {
            has_audio: true,
            ..MediaProbe::default()
        };
        let metadata = infer_music_metadata(
            &probe,
            Path::new("/OST/Example Artist/Example Album/CD1/01.flac"),
            Path::new("/OST"),
            "01.flac",
        );

        assert_eq!(metadata.0.as_deref(), Some("Example Artist"));
        assert_eq!(metadata.1.as_deref(), Some("Example Album"));
    }

    #[test]
    fn generic_episode_filename_uses_cleaned_directory_title() {
        let parsed = parse_name("Episode 01.mkv");
        let detected = infer_video_series(
            Path::new("/library/[Group] Example Show [BDRip 1080p]/Season 01/Episode 01.mkv"),
            Path::new("/library"),
            &parsed,
        );

        assert_eq!(detected.0.as_deref(), Some("Example Show"));
        assert_eq!(detected.1.as_deref(), Some("directory"));
    }

    #[test]
    fn numeric_episode_uses_selected_root_directory_title() {
        let parsed = parse_name("01.mkv");
        let detected = infer_video_series(
            Path::new("/library/Example Show/01.mkv"),
            Path::new("/library/Example Show"),
            &parsed,
        );

        assert_eq!(detected.0.as_deref(), Some("Example Show"));
        assert_eq!(detected.1.as_deref(), Some("directory"));
    }

    #[test]
    fn episode_directory_is_skipped_when_detecting_series() {
        let parsed = parse_name("Episode 01.mkv");
        let detected = infer_video_series(
            Path::new("/library/Example Show/S01E01/Episode 01.mkv"),
            Path::new("/library"),
            &parsed,
        );

        assert_eq!(detected.0.as_deref(), Some("Example Show"));
        assert_eq!(detected.1.as_deref(), Some("directory"));
    }

    #[test]
    fn directory_title_strips_order_season_and_release_tags() {
        assert_eq!(
            clean_directory_title("01 - Fate strange Fake Season 2 [WEB-DL 1080p]").as_deref(),
            Some("Fate Strange Fake")
        );
        assert_eq!(
            clean_directory_title("Kara no Kyoukai - BD-BOX Disc8 Remix (BD 12").as_deref(),
            Some("Kara no Kyoukai")
        );
        assert_eq!(
            clean_directory_title("Example Show 第2季").as_deref(),
            Some("Example Show")
        );
        assert_eq!(
            clean_directory_title("Example Show Part 2").as_deref(),
            Some("Example Show")
        );
        assert_eq!(
            clean_directory_title("86 Eighty-Six").as_deref(),
            Some("86 Eighty-Six")
        );
    }

    #[test]
    fn nested_series_directory_can_define_family() {
        assert_eq!(
            infer_series_family(
                Path::new("/library/Fate/Fate strange Fake/Episode 01.mkv"),
                Path::new("/library"),
                "Fate Strange Fake"
            )
            .as_deref(),
            Some("Fate")
        );
    }

    #[test]
    fn nested_roots_and_exclusions_are_normalized() {
        let prepared = prepare_scan_config(
            vec!["/library".to_string(), "/library/anime".to_string()],
            vec![
                "/library/anime/extras".to_string(),
                "/library/anime/extras/bonus".to_string(),
            ],
        )
        .expect("valid scan config");

        assert_eq!(prepared.roots.len(), 1);
        assert_eq!(prepared.excluded_paths.len(), 1);
        assert!(
            prepare_scan_config(vec!["/library".to_string()], vec!["/outside".to_string()])
                .is_err()
        );
    }

    #[test]
    fn root_commit_only_replaces_the_completed_root() {
        let conn = Connection::open_in_memory().expect("in-memory database");
        init_schema(&conn).expect("schema");
        insert_video(&conn, "/offline/show/01.mkv", "/offline", "Offline Show");
        insert_video(&conn, "/online/show/01.mkv", "/online", "Online Show");
        let root = RootSpec {
            path: PathBuf::from("/online"),
            display_path: "/online".to_string(),
            key: normalized_path_string(Path::new("/online")),
        };

        commit_root_scan(&conn, 1, &root, &[], &[]).expect("commit root");
        let offline_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media_files WHERE root_key = ?1",
                params![normalized_path_string(Path::new("/offline"))],
                |row| row.get(0),
            )
            .expect("offline count");
        let online_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media_files WHERE root_key = ?1",
                params![normalized_path_string(Path::new("/online"))],
                |row| row.get(0),
            )
            .expect("online count");

        assert_eq!(offline_count, 1);
        assert_eq!(online_count, 0);
    }

    #[test]
    fn stopped_scan_persists_pending_skip_details() {
        let conn = Connection::open_in_memory().expect("in-memory database");
        init_schema(&conn).expect("schema");
        let skip = PendingSkip {
            path: PathBuf::from("/library/broken.mkv"),
            root_path: PathBuf::from("/library"),
            root_key: normalized_path_string(Path::new("/library")),
            reason: "ffprobe_error".to_string(),
            detail: "failed".to_string(),
            is_short_video: false,
        };

        persist_pending_skips_on_stop(&conn, 7, &[skip]).expect("persist skips");
        let skips = load_scan_skips(&conn, 7, false).expect("load skips");
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0].reason, "ffprobe_error");
    }

    #[test]
    fn scan_history_preserves_status_and_rejects_corrupt_json() {
        let conn = Connection::open_in_memory().expect("in-memory database");
        init_schema(&conn).expect("schema");
        let scan_id = create_scan_run(
            &conn,
            100,
            &["/library".to_string()],
            &["/library/extras".to_string()],
        )
        .expect("create scan");
        let summary = ScanSummary {
            scan_id,
            started_at_ms: 100,
            completed_at_ms: 250,
            duration_ms: 150,
            scanned_files: 2,
            imported_files: 1,
            skipped_files: 1,
            skipped_short_files: 0,
            recorded_directories: 1,
            ffprobe_missing: false,
            status: "partial".to_string(),
            failed_roots: vec![FailedRoot {
                path: "/offline".to_string(),
                detail: "unavailable".to_string(),
            }],
        };
        finish_scan_run(&conn, &summary, None).expect("finish scan");

        let history = load_scan_history(&conn, 10).expect("load history");
        assert_eq!(history[0].status, "partial");
        assert_eq!(history[0].failed_roots.len(), 1);

        conn.execute(
            "UPDATE scan_runs SET paths_json = 'invalid' WHERE id = ?1",
            params![scan_id],
        )
        .expect("corrupt history");
        assert!(load_scan_history(&conn, 10).is_err());
    }

    #[test]
    fn legacy_scan_skips_are_migrated_without_losing_rows() {
        let conn = Connection::open_in_memory().expect("in-memory database");
        conn.execute_batch(
            r#"
            CREATE TABLE scan_skips (
              id INTEGER PRIMARY KEY,
              path TEXT NOT NULL UNIQUE,
              file_name TEXT NOT NULL,
              root_path TEXT NOT NULL,
              reason TEXT NOT NULL,
              detail TEXT NOT NULL,
              is_short_video INTEGER NOT NULL DEFAULT 0,
              file_size INTEGER,
              modified_ms INTEGER,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
              updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX idx_scan_skips_root ON scan_skips(root_path);
            CREATE INDEX idx_scan_skips_short ON scan_skips(is_short_video);
            INSERT INTO scan_skips (
              path, file_name, root_path, reason, detail, is_short_video
            ) VALUES ('/library/broken.mkv', 'broken.mkv', '/library', 'ffprobe_error', 'failed', 0);
            "#,
        )
        .expect("legacy schema");

        init_schema(&conn).expect("migrate schema");
        assert!(table_has_column(&conn, "scan_skips", "scan_id").expect("scan_id column"));
        let migrated: (Option<i64>, String, String) = conn
            .query_row(
                "SELECT scan_id, path, root_key FROM scan_skips",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("migrated row");
        assert_eq!(migrated.0, None);
        assert_eq!(migrated.1, "/library/broken.mkv");
        assert_eq!(migrated.2, normalized_path_string(Path::new("/library")));
        let schema_version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(schema_version, DATABASE_SCHEMA_VERSION);

        init_schema(&conn).expect("idempotent schema initialization");
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM scan_skips", [], |row| row.get(0))
            .expect("migrated row count");
        assert_eq!(row_count, 1);
    }

    #[test]
    fn library_summary_and_file_pages_do_not_duplicate_startup_payload() {
        let conn = Connection::open_in_memory().expect("in-memory database");
        init_schema(&conn).expect("schema");
        for episode in 1..=205 {
            insert_video(
                &conn,
                &format!("/library/show/{episode:03}.mkv"),
                "/library",
                "Example Show",
            );
        }

        let library = load_library(&conn, None).expect("load summaries");
        assert_eq!(library.video_directories[0].file_count, 205);
        assert_eq!(library.video_series[0].file_count, 205);
        let page = load_resource_page(
            &conn,
            ResourceQuery {
                kind: "video_series".to_string(),
                media_kind: "video".to_string(),
                key: library.video_series[0].key.clone(),
                source_keys: library.video_series[0].resource_keys.clone(),
                offset: 0,
                limit: 200,
            },
        )
        .expect("load first page");

        assert_eq!(page.total, 205);
        assert_eq!(page.files.len(), 200);
    }

    #[test]
    fn file_search_keeps_music_and_video_directories_separate() {
        let conn = Connection::open_in_memory().expect("in-memory database");
        init_schema(&conn).expect("schema");
        insert_video(
            &conn,
            "/library/shared/unique-video.mkv",
            "/library",
            "Example Show",
        );
        insert_music(
            &conn,
            "/library/shared/unique-audio.flac",
            "/library",
            "Example Artist",
        );

        let result = load_library(&conn, Some("unique-video")).expect("search library");
        assert_eq!(result.video_directories.len(), 1);
        assert!(result.music_directories.is_empty());
    }
}
