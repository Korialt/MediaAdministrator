import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import type { LibraryData, MediaDirectory, MediaGroup, ResourceVariant, ScanProgress, ScanSummary } from "./types";

const EMPTY_LIBRARY: LibraryData = { musicDirectories: [], videoDirectories: [], musicArtists: [], videoSeries: [] };

type Theme = "light" | "dark";
type ViewMode = "list" | "grid";
type ActiveEntry = "home" | "music" | "video";
type MusicBrowseMode = "directory" | "artist";
type VideoBrowseMode = "directory" | "series";
type MergeKind = "music_artist" | "video_series";

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function formatDuration(seconds: number | null): string {
  if (seconds == null || !Number.isFinite(seconds)) return "-";
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function episodeLabel(file: ResourceVariant): string | null {
  if (file.seasonNumber == null || file.episodeNumber == null) return null;
  return `S${file.seasonNumber.toString().padStart(2, "0")}E${file.episodeNumber.toString().padStart(2, "0")}`;
}

function scanStatusText(progress: ScanProgress): string {
  if (progress.phase === "discovering") {
    return `正在统计目录媒体文件：已发现 ${progress.discoveredFiles} 个`;
  }

  const total = progress.totalFiles ?? progress.discoveredFiles;
  return `后台分析媒体文件：${progress.processedFiles} / ${total}`;
}

function progressPercent(progress: ScanProgress | null): number | null {
  if (!progress || progress.phase !== "processing" || !progress.totalFiles) return null;
  return Math.min(100, Math.round((progress.processedFiles / progress.totalFiles) * 100));
}

function directoryMatches(directory: MediaDirectory, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  if (directory.name.toLowerCase().includes(text)) return true;
  if (directory.relativePath.toLowerCase().includes(text)) return true;
  if (directory.path.toLowerCase().includes(text)) return true;
  return directory.files.some((file) => file.fileName.toLowerCase().includes(text) || file.path.toLowerCase().includes(text));
}

function groupMatches(group: MediaGroup, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  if (group.name.toLowerCase().includes(text)) return true;
  if (group.subtitle?.toLowerCase().includes(text)) return true;
  if (group.sourceKeys.some((key) => key.toLowerCase().includes(text))) return true;
  if (group.files.some((file) => file.fileName.toLowerCase().includes(text) || file.path.toLowerCase().includes(text))) return true;
  return group.childGroups.some((child) => groupMatches(child, query));
}

function fileSpec(file: ResourceVariant): string {
  const parts = [file.resolution, file.videoCodec, file.audioCodec].filter(Boolean);
  return parts.length > 0 ? parts.join(" / ") : file.container ?? "-";
}

function directoryTitle(directory: MediaDirectory): string {
  return directory.relativePath || directory.name;
}

function fileMeta(file: ResourceVariant): string {
  const parts = [episodeLabel(file), file.releaseGroup, file.source, file.musicArtist, file.musicAlbum, file.seriesTitle].filter(Boolean);
  return parts.join(" / ") || file.musicTitle || file.titleGuess;
}

function FileRows({ files }: { files: ResourceVariant[] }) {
  return (
    <div className="file-table-wrap">
      <table className="file-table">
        <thead>
          <tr>
            <th>文件</th>
            <th>规格</th>
            <th>大小</th>
            <th>时长</th>
            <th>路径</th>
          </tr>
        </thead>
        <tbody>
          {files.map((file) => (
            <tr key={file.id}>
              <td>
                <div className="file-name">{file.fileName}</div>
                <div className="muted">{fileMeta(file)}</div>
              </td>
              <td>{fileSpec(file)}</td>
              <td>{formatBytes(file.fileSize)}</td>
              <td>{formatDuration(file.durationSeconds)}</td>
              <td>
                <code title={file.path}>{file.path}</code>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function DirectoryItem({ directory, mode }: { directory: MediaDirectory; mode: ViewMode }) {
  return (
    <details className={mode === "grid" ? "directory-card" : "directory-row"}>
      <summary>
        <div className="directory-main">
          <strong>{directoryTitle(directory)}</strong>
          {directory.parentName ? <span>{directory.parentName}</span> : null}
          <code title={directory.path}>{directory.path}</code>
        </div>
        <b>
          {directory.fileCount} 个文件 · {formatBytes(directory.totalSize)}
        </b>
      </summary>
      <FileRows files={directory.files} />
    </details>
  );
}

function MergeControls({ group, mergeKind, onMerge }: { group: MediaGroup; mergeKind: MergeKind; onMerge: (kind: MergeKind, group: MediaGroup, targetName: string) => Promise<void> }) {
  const [targetName, setTargetName] = useState(group.name);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setTargetName(group.name);
  }, [group.key, group.name]);

  async function submit(target: string) {
    setBusy(true);
    try {
      await onMerge(mergeKind, group, target);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="merge-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit(targetName);
      }}
    >
      <label>
        合并到
        <input value={targetName} onChange={(event) => setTargetName(event.target.value)} placeholder="目标名称" />
      </label>
      <button type="submit" className="primary" disabled={busy || targetName.trim().length === 0}>
        合并
      </button>
      <button type="button" disabled={busy} onClick={() => void submit("")}>
        清除规则
      </button>
    </form>
  );
}

function GroupItem({ group, mode, mergeKind, onMerge, nested = false }: { group: MediaGroup; mode: ViewMode; mergeKind: MergeKind; onMerge: (kind: MergeKind, group: MediaGroup, targetName: string) => Promise<void>; nested?: boolean }) {
  const hasChildren = group.childGroups.length > 0;

  return (
    <details className={`${mode === "grid" && !nested ? "directory-card" : "directory-row"}${nested ? " nested-group" : ""}`}>
      <summary>
        <div className="directory-main">
          <strong>{group.name}</strong>
          {group.subtitle ? <span>{group.subtitle}</span> : null}
          <code title={group.sourceKeys.join("\n")}>{hasChildren ? `${group.childGroups.length} 个子系列` : `${group.sourceKeys.length} 个识别来源`}</code>
        </div>
        <b>
          {group.fileCount} 个文件 · {formatBytes(group.totalSize)}
        </b>
      </summary>
      <MergeControls group={group} mergeKind={mergeKind} onMerge={onMerge} />
      {hasChildren ? (
        <div className="child-group-list">
          {group.childGroups.map((child) => (
            <GroupItem group={child} key={child.key} mode="list" mergeKind={mergeKind} onMerge={onMerge} nested />
          ))}
        </div>
      ) : (
        <FileRows files={group.files} />
      )}
    </details>
  );
}

function DirectorySection({
  title,
  emptyText,
  directories,
  mode,
}: {
  title: string;
  emptyText: string;
  directories: MediaDirectory[];
  mode: ViewMode;
}) {
  return (
    <section className="library-section">
      <header className="section-header">
        <h3>{title}</h3>
        <span>{directories.length} 个目录</span>
      </header>

      {directories.length === 0 ? <div className="empty">{emptyText}</div> : null}

      <div className={mode === "grid" ? "directory-grid" : "directory-list"}>
        {directories.map((directory) => (
          <DirectoryItem directory={directory} key={directory.key} mode={mode} />
        ))}
      </div>
    </section>
  );
}

function GroupSection({
  title,
  emptyText,
  groups,
  mode,
  mergeKind,
  onMerge,
}: {
  title: string;
  emptyText: string;
  groups: MediaGroup[];
  mode: ViewMode;
  mergeKind: MergeKind;
  onMerge: (kind: MergeKind, group: MediaGroup, targetName: string) => Promise<void>;
}) {
  return (
    <section className="library-section">
      <header className="section-header">
        <h3>{title}</h3>
        <span>{groups.length} 个分类</span>
      </header>

      {groups.length === 0 ? <div className="empty">{emptyText}</div> : null}

      <div className={mode === "grid" ? "directory-grid" : "directory-list"}>
        {groups.map((group) => (
          <GroupItem group={group} key={group.key} mode={mode} mergeKind={mergeKind} onMerge={onMerge} />
        ))}
      </div>
    </section>
  );
}

function EntryPanel({
  musicCount,
  videoCount,
  musicFiles,
  videoFiles,
  musicArtists,
  videoSeries,
  onSelect,
}: {
  musicCount: number;
  videoCount: number;
  musicFiles: number;
  videoFiles: number;
  musicArtists: number;
  videoSeries: number;
  onSelect: (entry: ActiveEntry) => void;
}) {
  return (
    <section className="entry-panel">
      <button type="button" className="entry-card" onClick={() => onSelect("music")}>
        <span>音乐</span>
        <strong>{musicCount}</strong>
        <small>
          {musicFiles} 个音频文件 · {musicArtists} 位作者
        </small>
      </button>
      <button type="button" className="entry-card" onClick={() => onSelect("video")}>
        <span>影视</span>
        <strong>{videoCount}</strong>
        <small>
          {videoFiles} 个视频文件 · {videoSeries} 个系列
        </small>
      </button>
    </section>
  );
}

export default function App() {
  const [paths, setPaths] = useState<string[]>([]);
  const [excludedPaths, setExcludedPaths] = useState<string[]>([]);
  const [library, setLibrary] = useState<LibraryData>(EMPTY_LIBRARY);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("正在检查 ffprobe...");
  const [isScanning, setIsScanning] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [lastScan, setLastScan] = useState<ScanSummary | null>(null);
  const [theme, setTheme] = useState<Theme>(() => (window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light"));
  const [viewMode, setViewMode] = useState<ViewMode>("list");
  const [activeEntry, setActiveEntry] = useState<ActiveEntry>("home");
  const [musicBrowseMode, setMusicBrowseMode] = useState<MusicBrowseMode>("directory");
  const [videoBrowseMode, setVideoBrowseMode] = useState<VideoBrowseMode>("directory");

  async function refreshLibrary() {
    const data = await invoke<LibraryData>("list_library");
    setLibrary(data);
  }

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    invoke<string>("check_ffprobe")
      .then((message) => setStatus(message))
      .catch((error) => setStatus(String(error)));
    refreshLibrary().catch((error) => setStatus(String(error)));
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlistenProgress: UnlistenFn | null = null;
    let unlistenComplete: UnlistenFn | null = null;
    let unlistenError: UnlistenFn | null = null;

    listen<ScanProgress>("scan-progress", (event) => {
      const progress = event.payload;
      setIsScanning(true);
      setScanProgress(progress);
      setStatus(scanStatusText(progress));
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenProgress = unlisten;
    });

    listen<ScanSummary>("scan-complete", (event) => {
      const summary = event.payload;
      setLastScan(summary);
      setLibrary(summary.library);
      setIsScanning(false);
      setIsStopping(false);
      setScanProgress({
        phase: "processing",
        discoveredFiles: summary.scannedFiles,
        processedFiles: summary.scannedFiles,
        importedFiles: summary.importedFiles,
        skippedFiles: summary.skippedFiles,
        skippedShortFiles: summary.skippedShortFiles,
        totalFiles: summary.scannedFiles,
        currentPath: null,
        ffprobeMissing: summary.ffprobeMissing,
      });
      setStatus(summary.ffprobeMissing ? "扫描完成，但没有找到 ffprobe，短视频过滤可能不完整" : "扫描完成");
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenComplete = unlisten;
    });

    listen<string>("scan-error", (event) => {
      setIsScanning(false);
      setIsStopping(false);
      setStatus(String(event.payload));
      refreshLibrary().catch((error) => setStatus(String(error)));
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenError = unlisten;
    });

    return () => {
      disposed = true;
      unlistenProgress?.();
      unlistenComplete?.();
      unlistenError?.();
    };
  }, []);

  async function chooseDirectories() {
    const selected = await open({
      directory: true,
      multiple: true,
      title: "选择媒体目录",
    });

    if (!selected) return;
    const next = Array.isArray(selected) ? selected : [selected];
    setPaths((current) => Array.from(new Set([...current, ...next])));
  }

  async function chooseExcludedDirectories() {
    const selected = await open({
      directory: true,
      multiple: true,
      title: "选择排除子目录",
    });

    if (!selected) return;
    const next = Array.isArray(selected) ? selected : [selected];
    setExcludedPaths((current) => Array.from(new Set([...current, ...next])));
  }

  function removeExcludedPath(path: string) {
    setExcludedPaths((current) => current.filter((item) => item !== path));
  }

  async function scan() {
    if (paths.length === 0) return;
    setIsScanning(true);
    setIsStopping(false);
    setScanProgress({
      phase: "discovering",
      discoveredFiles: 0,
      processedFiles: 0,
      importedFiles: 0,
      skippedFiles: 0,
      skippedShortFiles: 0,
      totalFiles: null,
      currentPath: null,
      ffprobeMissing: false,
    });
    setStatus("后台扫描已启动...");
    try {
      await invoke<void>("start_scan", { paths, excludedPaths });
    } catch (error) {
      setStatus(String(error));
      setIsScanning(false);
    }
  }

  async function stopScan() {
    setIsStopping(true);
    setStatus("正在停止扫描...");
    try {
      await invoke<void>("stop_scan");
    } catch (error) {
      setIsStopping(false);
      setStatus(String(error));
    }
  }

  async function applyMerge(kind: MergeKind, group: MediaGroup, targetName: string) {
    setStatus(targetName.trim() ? "正在保存合并规则..." : "正在清除合并规则...");
    try {
      const data = await invoke<LibraryData>("set_merge_rules", {
        request: {
          kind,
          sourceKeys: group.sourceKeys,
          targetName,
        },
      });
      setLibrary(data);
      setStatus(targetName.trim() ? "合并规则已保存" : "合并规则已清除");
    } catch (error) {
      setStatus(String(error));
    }
  }

  const musicDirectories = useMemo(
    () => library.musicDirectories.filter((directory) => directoryMatches(directory, query)),
    [library, query],
  );
  const videoDirectories = useMemo(
    () => library.videoDirectories.filter((directory) => directoryMatches(directory, query)),
    [library, query],
  );
  const musicArtists = useMemo(() => library.musicArtists.filter((group) => groupMatches(group, query)), [library, query]);
  const videoSeries = useMemo(() => library.videoSeries.filter((group) => groupMatches(group, query)), [library, query]);

  const totals = useMemo(() => {
    const musicFiles = library.musicDirectories.reduce((sum, directory) => sum + directory.fileCount, 0);
    const videoFiles = library.videoDirectories.reduce((sum, directory) => sum + directory.fileCount, 0);
    const bytes = [...library.musicDirectories, ...library.videoDirectories].reduce((sum, directory) => sum + directory.totalSize, 0);
    return {
      musicDirectories: library.musicDirectories.length,
      videoDirectories: library.videoDirectories.length,
      musicArtists: library.musicArtists.length,
      videoSeries: library.videoSeries.length,
      musicFiles,
      videoFiles,
      files: musicFiles + videoFiles,
      bytes,
    };
  }, [library]);

  const percent = progressPercent(scanProgress);
  const activeBrowseMode = activeEntry === "music" ? musicBrowseMode : videoBrowseMode;
  const activeTitle =
    activeEntry === "music"
      ? musicBrowseMode === "directory"
        ? "音乐目录"
        : "音乐作者"
      : videoBrowseMode === "directory"
        ? "影视目录"
        : "影视系列";
  const activeEmptyText = activeEntry === "music" ? "暂无音乐资源。" : "暂无影视资源。";

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div>
            <h1>Media Administrator</h1>
            <p>媒体索引</p>
          </div>
          <button type="button" className="theme-toggle" onClick={() => setTheme((current) => (current === "dark" ? "light" : "dark"))}>
            {theme === "dark" ? "浅色" : "暗黑"}
          </button>
        </div>

        <section className="panel">
          <div className="panel-title">扫描目录</div>
          <div className="path-list">
            {paths.length === 0 ? <span className="muted">未选择目录</span> : null}
            {paths.map((path) => (
              <div className="path-chip" key={path} title={path}>
                {path}
              </div>
            ))}
          </div>
          <div className="button-row">
            <button type="button" onClick={chooseDirectories}>
              选择目录
            </button>
            {isScanning ? (
              <button type="button" className="danger" disabled={isStopping} onClick={stopScan}>
                停止
              </button>
            ) : (
              <button type="button" className="primary" disabled={paths.length === 0} onClick={scan}>
                扫描
              </button>
            )}
          </div>

          <div className="exclude-header">
            <span>排除子目录</span>
            <button type="button" onClick={chooseExcludedDirectories}>
              添加排除
            </button>
          </div>
          <div className="path-list compact">
            {excludedPaths.length === 0 ? <span className="muted">未设置排除目录</span> : null}
            {excludedPaths.map((path) => (
              <div className="path-chip removable" key={path} title={path}>
                <span>{path}</span>
                <button type="button" onClick={() => removeExcludedPath(path)}>
                  移除
                </button>
              </div>
            ))}
          </div>
        </section>

        <section className="stats">
          <div>
            <span>{totals.musicDirectories}</span>
            <small>音乐目录</small>
          </div>
          <div>
            <span>{totals.videoDirectories}</span>
            <small>影视目录</small>
          </div>
          <div>
            <span>{totals.musicArtists}</span>
            <small>作者</small>
          </div>
          <div>
            <span>{totals.videoSeries}</span>
            <small>系列</small>
          </div>
          <div>
            <span>{totals.files}</span>
            <small>文件</small>
          </div>
          <div>
            <span>{formatBytes(totals.bytes)}</span>
            <small>容量</small>
          </div>
        </section>

        <section className="panel">
          <div className="panel-title">状态</div>
          <p className="status">{status}</p>
          {scanProgress ? (
            <div className="progress-block">
              <div className="progress-bar" data-indeterminate={percent == null && isScanning ? "true" : "false"}>
                <div className="progress-fill" style={{ width: `${percent ?? 42}%` }} />
              </div>
              <div className="progress-meta">
                <span>发现 {scanProgress.discoveredFiles}</span>
                <span>已处理 {scanProgress.processedFiles}</span>
                <span>已入库 {scanProgress.importedFiles}</span>
                <span>短视频过滤 {scanProgress.skippedShortFiles}</span>
              </div>
              {scanProgress.currentPath ? <code className="current-path">{scanProgress.currentPath}</code> : null}
            </div>
          ) : null}
          {lastScan ? (
            <p className="muted">
              最近扫描：入库 {lastScan.importedFiles} 个媒体文件，记录 {lastScan.recordedDirectories} 个目录，短视频过滤 {lastScan.skippedShortFiles} 个
            </p>
          ) : null}
        </section>
      </aside>

      <section className="content">
        <header className="toolbar">
          <div>
            <h2>{activeEntry === "home" ? "媒体库" : activeTitle}</h2>
            <p>{activeEntry === "home" ? "选择入口" : activeBrowseMode === "directory" ? "目录资源" : "分类资源"}</p>
          </div>
          {activeEntry !== "home" ? (
            <div className="toolbar-actions">
              <button type="button" onClick={() => setActiveEntry("home")}>
                返回入口
              </button>
              <div className="view-switch" aria-label="分类方式">
                {activeEntry === "music" ? (
                  <>
                    <button type="button" className={musicBrowseMode === "directory" ? "active" : ""} onClick={() => setMusicBrowseMode("directory")}>
                      目录
                    </button>
                    <button type="button" className={musicBrowseMode === "artist" ? "active" : ""} onClick={() => setMusicBrowseMode("artist")}>
                      作者
                    </button>
                  </>
                ) : (
                  <>
                    <button type="button" className={videoBrowseMode === "directory" ? "active" : ""} onClick={() => setVideoBrowseMode("directory")}>
                      目录
                    </button>
                    <button type="button" className={videoBrowseMode === "series" ? "active" : ""} onClick={() => setVideoBrowseMode("series")}>
                      系列
                    </button>
                  </>
                )}
              </div>
              <div className="view-switch" aria-label="显示格式">
                <button type="button" className={viewMode === "list" ? "active" : ""} onClick={() => setViewMode("list")}>
                  列表
                </button>
                <button type="button" className={viewMode === "grid" ? "active" : ""} onClick={() => setViewMode("grid")}>
                  图表
                </button>
              </div>
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={activeBrowseMode === "directory" ? "搜索目录或文件路径" : "搜索分类或文件路径"}
                aria-label="搜索"
              />
            </div>
          ) : null}
        </header>

        {activeEntry === "home" ? (
          <EntryPanel
            musicCount={totals.musicDirectories}
            videoCount={totals.videoDirectories}
            musicFiles={totals.musicFiles}
            videoFiles={totals.videoFiles}
            musicArtists={totals.musicArtists}
            videoSeries={totals.videoSeries}
            onSelect={setActiveEntry}
          />
        ) : activeEntry === "music" && musicBrowseMode === "artist" ? (
          <GroupSection title="音乐作者" emptyText={activeEmptyText} groups={musicArtists} mode={viewMode} mergeKind="music_artist" onMerge={applyMerge} />
        ) : activeEntry === "video" && videoBrowseMode === "series" ? (
          <GroupSection title="影视系列" emptyText={activeEmptyText} groups={videoSeries} mode={viewMode} mergeKind="video_series" onMerge={applyMerge} />
        ) : (
          <DirectorySection
            title={activeTitle}
            emptyText={activeEmptyText}
            directories={activeEntry === "music" ? musicDirectories : videoDirectories}
            mode={viewMode}
          />
        )}
      </section>
    </main>
  );
}
