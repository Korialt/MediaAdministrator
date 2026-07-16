import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import type {
  LibraryData,
  MediaDirectory,
  MediaGroup,
  ResourcePage,
  ResourceVariant,
  ScanConfig,
  ScanFailureEvent,
  ScanProgress,
  ScanRun,
  ScanRunStatus,
  ScanSkip,
  ScanSummary,
} from "./types";

const EMPTY_LIBRARY: LibraryData = { musicDirectories: [], videoDirectories: [], musicArtists: [], videoSeries: [] };

type Theme = "light" | "dark";
type ViewMode = "list" | "grid";
type ActiveEntry = "home" | "music" | "video";
type MusicBrowseMode = "directory" | "artist";
type VideoBrowseMode = "directory" | "series";
type MergeKind = "music_artist" | "video_series" | "video_family";

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

function formatElapsedMs(milliseconds: number | null): string {
  if (milliseconds == null || !Number.isFinite(milliseconds) || milliseconds < 0) return "-";
  const totalSeconds = Math.floor(milliseconds / 1000);
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = totalSeconds % 60;
  if (h > 0) return `${h}时${m.toString().padStart(2, "0")}分${s.toString().padStart(2, "0")}秒`;
  if (m > 0) return `${m}分${s.toString().padStart(2, "0")}秒`;
  return `${s}秒`;
}

function formatClock(milliseconds: number | null): string {
  if (milliseconds == null || !Number.isFinite(milliseconds) || milliseconds <= 0) return "-";
  return new Date(milliseconds).toLocaleTimeString();
}

function formatDateTime(milliseconds: number | null): string {
  if (milliseconds == null || !Number.isFinite(milliseconds) || milliseconds <= 0) return "-";
  return new Date(milliseconds).toLocaleString();
}

function phaseLabel(phase: ScanProgress["phase"]): string {
  return phase === "discovering" ? "统计文件" : "分析媒体";
}

function processingSpeed(progress: ScanProgress | null, nowMs: number): string {
  if (!progress || progress.processedFiles === 0) return "-";
  const elapsedSeconds = Math.max(1, (nowMs - progress.scanStartedAtMs) / 1000);
  return `${(progress.processedFiles / elapsedSeconds).toFixed(2)} 个/秒`;
}

function skipReasonLabel(reason: string): string {
  if (reason === "ffprobe_error") return "ffprobe 分析失败";
  if (reason === "metadata_error") return "元数据读取失败";
  if (reason === "no_media_stream") return "没有有效媒体流";
  if (reason === "invalid_path") return "路径无效";
  if (reason === "short_video") return "短视频";
  return reason || "未知原因";
}

function scanRunStatusLabel(status: ScanRunStatus): string {
  if (status === "running") return "运行中";
  if (status === "completed") return "已完成";
  if (status === "partial") return "部分完成";
  if (status === "stopped") return "已停止";
  return "失败";
}

function directoryMatches(directory: MediaDirectory, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  if (directory.name.toLowerCase().includes(text)) return true;
  if (directory.relativePath.toLowerCase().includes(text)) return true;
  if (directory.path.toLowerCase().includes(text)) return true;
  return false;
}

function groupMatches(group: MediaGroup, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  if (group.name.toLowerCase().includes(text)) return true;
  if (group.subtitle?.toLowerCase().includes(text)) return true;
  if (group.sourceKeys.some((key) => key.toLowerCase().includes(text))) return true;
  return group.childGroups.some((child) => groupMatches(child, query));
}

function fileSpec(file: ResourceVariant): string {
  const parts = [file.resolution, file.videoCodec, file.audioCodec].filter(Boolean);
  return parts.length > 0 ? parts.join(" / ") : file.container ?? "-";
}

function directoryTitle(directory: MediaDirectory): string {
  return directory.relativePath || directory.name;
}

function normalizedPathForComparison(value: string): string {
  return value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

function pathIsWithin(candidate: string, parent: string): boolean {
  const candidatePath = normalizedPathForComparison(candidate);
  const parentPath = normalizedPathForComparison(parent);
  return candidatePath === parentPath || candidatePath.startsWith(`${parentPath}/`);
}

function pathIsStrictlyWithin(candidate: string, parent: string): boolean {
  const candidatePath = normalizedPathForComparison(candidate);
  const parentPath = normalizedPathForComparison(parent);
  return candidatePath.startsWith(`${parentPath}/`);
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

type ResourceLoadRequest = {
  kind: "directory" | "music_artist" | "video_series";
  mediaKind: "music" | "video";
  key: string;
  sourceKeys: string[];
};

function LazyFileRows({ active, request }: { active: boolean; request: ResourceLoadRequest }) {
  const [files, setFiles] = useState<ResourceVariant[]>([]);
  const [total, setTotal] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestIdRef = useRef(0);
  const loadingRef = useRef(false);
  const requestKey = `${request.kind}:${request.mediaKind}:${request.key}:${request.sourceKeys.join("|")}`;

  async function loadPage(offset: number, replace: boolean) {
    if (loadingRef.current) return;
    const requestId = ++requestIdRef.current;
    loadingRef.current = true;
    setLoading(true);
    setError(null);
    try {
      const page = await invoke<ResourcePage>("list_resources", {
        request: {
          ...request,
          offset,
          limit: 200,
        },
      });
      if (requestId !== requestIdRef.current) return;
      setFiles((current) => (replace ? page.files : [...current, ...page.files]));
      setTotal(page.total);
      setLoaded(true);
    } catch (loadError) {
      if (requestId !== requestIdRef.current) return;
      setError(String(loadError));
      setLoaded(true);
    } finally {
      if (requestId === requestIdRef.current) {
        loadingRef.current = false;
        setLoading(false);
      }
    }
  }

  useEffect(() => {
    requestIdRef.current += 1;
    loadingRef.current = false;
    setFiles([]);
    setTotal(0);
    setLoaded(false);
    setLoading(false);
    setError(null);
    return () => {
      requestIdRef.current += 1;
      loadingRef.current = false;
    };
  }, [requestKey]);

  useEffect(() => {
    if (active && !loaded && !loading) void loadPage(0, true);
  }, [active, loaded, loading, requestKey]);

  if (!active) return null;
  if (!loaded && loading) return <div className="inline-state">正在读取文件...</div>;
  if (error) {
    return (
      <div className="inline-state error-state">
        <span>{error}</span>
        <button type="button" onClick={() => void loadPage(0, true)}>
          重试
        </button>
      </div>
    );
  }
  if (loaded && files.length === 0) return <div className="inline-state">当前分类没有可显示的文件。</div>;

  return (
    <>
      <FileRows files={files} />
      {files.length < total ? (
        <button type="button" className="load-more" disabled={loading} onClick={() => void loadPage(files.length, false)}>
          {loading ? "读取中" : `加载更多（${files.length} / ${total}）`}
        </button>
      ) : null}
    </>
  );
}

function DirectoryItem({ directory, mode }: { directory: MediaDirectory; mode: ViewMode }) {
  const [open, setOpen] = useState(false);

  return (
    <details className={mode === "grid" ? "directory-card" : "directory-row"} onToggle={(event) => setOpen(event.currentTarget.open)}>
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
      <LazyFileRows
        active={open}
        request={{ kind: "directory", mediaKind: directory.mediaKind, key: directory.key, sourceKeys: [] }}
      />
    </details>
  );
}

function MergeControls({
  group,
  mergeKind,
  disabled,
  onMerge,
}: {
  group: MediaGroup;
  mergeKind: "music_artist" | "video_series";
  disabled: boolean;
  onMerge: (kind: MergeKind, group: MediaGroup, targetName: string) => Promise<void>;
}) {
  const [targetName, setTargetName] = useState(group.name);
  const [familyName, setFamilyName] = useState(group.familyName ?? "");
  const [busyKind, setBusyKind] = useState<MergeKind | null>(null);

  useEffect(() => {
    setTargetName(group.name);
    setFamilyName(group.familyName ?? "");
  }, [group.key, group.name, group.familyName]);

  async function submit(kind: MergeKind, target: string) {
    setBusyKind(kind);
    try {
      await onMerge(kind, group, target);
    } finally {
      setBusyKind(null);
    }
  }

  const busy = busyKind != null;
  return (
    <div className="merge-controls">
      <form
        className="merge-form"
        onSubmit={(event) => {
          event.preventDefault();
          void submit(mergeKind, targetName);
        }}
      >
        <label>
          合并到
          <input disabled={disabled || busy} value={targetName} onChange={(event) => setTargetName(event.target.value)} placeholder="目标名称" />
        </label>
        <button type="submit" className="primary" disabled={disabled || busy || targetName.trim().length === 0}>
          {busyKind === mergeKind ? "保存中" : "合并"}
        </button>
        <button type="button" disabled={disabled || busy} onClick={() => void submit(mergeKind, "")}>
          清除规则
        </button>
      </form>

      {mergeKind === "video_series" && group.childGroups.length === 0 ? (
        <form
          className="merge-form family-form"
          onSubmit={(event) => {
            event.preventDefault();
            void submit("video_family", familyName);
          }}
        >
          <label>
            作品族
            <input disabled={disabled || busy} value={familyName} onChange={(event) => setFamilyName(event.target.value)} placeholder="例如 Fate" />
          </label>
          <button type="submit" className="primary" disabled={disabled || busy || familyName.trim().length === 0}>
            {busyKind === "video_family" ? "保存中" : "设定"}
          </button>
          <button type="button" disabled={disabled || busy || group.familyName == null} onClick={() => void submit("video_family", "")}>
            清除作品族
          </button>
        </form>
      ) : null}
    </div>
  );
}

function GroupItem({
  group,
  mode,
  mergeKind,
  disabled,
  revision,
  onMerge,
  nested = false,
}: {
  group: MediaGroup;
  mode: ViewMode;
  mergeKind: "music_artist" | "video_series";
  disabled: boolean;
  revision: number;
  onMerge: (kind: MergeKind, group: MediaGroup, targetName: string) => Promise<void>;
  nested?: boolean;
}) {
  const hasChildren = group.childGroups.length > 0;
  const [open, setOpen] = useState(false);
  const mediaKind = mergeKind === "music_artist" ? "music" : "video";

  return (
    <details className={`${mode === "grid" && !nested ? "directory-card" : "directory-row"}${nested ? " nested-group" : ""}`} onToggle={(event) => setOpen(event.currentTarget.open)}>
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
      {open ? <MergeControls group={group} mergeKind={mergeKind} disabled={disabled} onMerge={onMerge} /> : null}
      {open && hasChildren ? (
        <div className="child-group-list">
          {group.childGroups.map((child) => (
            <GroupItem
              group={child}
              key={`${child.key}:${revision}`}
              mode="list"
              mergeKind={mergeKind}
              disabled={disabled}
              revision={revision}
              onMerge={onMerge}
              nested
            />
          ))}
        </div>
      ) : null}
      {!hasChildren ? (
        <LazyFileRows
          active={open}
          request={{
            kind: mergeKind,
            mediaKind,
            key: group.key,
            sourceKeys: group.resourceKeys,
          }}
        />
      ) : null}
    </details>
  );
}

function DirectorySection({
  title,
  emptyText,
  directories,
  mode,
  revision,
}: {
  title: string;
  emptyText: string;
  directories: MediaDirectory[];
  mode: ViewMode;
  revision: number;
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
          <DirectoryItem directory={directory} key={`${directory.key}:${revision}`} mode={mode} />
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
  disabled,
  revision,
  onMerge,
}: {
  title: string;
  emptyText: string;
  groups: MediaGroup[];
  mode: ViewMode;
  mergeKind: "music_artist" | "video_series";
  disabled: boolean;
  revision: number;
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
          <GroupItem
            group={group}
            key={`${group.key}:${revision}`}
            mode={mode}
            mergeKind={mergeKind}
            disabled={disabled}
            revision={revision}
            onMerge={onMerge}
          />
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
  const [searchResult, setSearchResult] = useState<{ query: string; data: LibraryData } | null>(null);
  const [query, setQuery] = useState("");
  const [systemStatus, setSystemStatus] = useState("正在初始化...");
  const [scanStatus, setScanStatus] = useState("扫描未启动");
  const [libraryStatus, setLibraryStatus] = useState("正在读取媒体库...");
  const [listenersReady, setListenersReady] = useState(false);
  const [isScanning, setIsScanning] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [isLoadingLibrary, setIsLoadingLibrary] = useState(false);
  const [isSearching, setIsSearching] = useState(false);
  const [libraryRevision, setLibraryRevision] = useState(0);
  const [isLoadingSkips, setIsLoadingSkips] = useState(false);
  const [skipListVisible, setSkipListVisible] = useState(false);
  const [selectedSkipScanId, setSelectedSkipScanId] = useState<number | null>(null);
  const [scanSkips, setScanSkips] = useState<ScanSkip[]>([]);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const [scanHistoryVisible, setScanHistoryVisible] = useState(false);
  const [scanHistory, setScanHistory] = useState<ScanRun[]>([]);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [lastScan, setLastScan] = useState<ScanSummary | null>(null);
  const [theme, setTheme] = useState<Theme>(() => (window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light"));
  const [viewMode, setViewMode] = useState<ViewMode>("list");
  const [activeEntry, setActiveEntry] = useState<ActiveEntry>("home");
  const [musicBrowseMode, setMusicBrowseMode] = useState<MusicBrowseMode>("directory");
  const [videoBrowseMode, setVideoBrowseMode] = useState<VideoBrowseMode>("directory");
  const [nowMs, setNowMs] = useState(() => Date.now());
  const scanStartedAtRef = useRef<number | null>(null);
  const libraryRequestIdRef = useRef(0);

  async function refreshLibrary(successStatus?: string) {
    const requestId = ++libraryRequestIdRef.current;
    setIsLoadingLibrary(true);
    setLibraryStatus(successStatus ? "正在刷新媒体库..." : "正在读取媒体库...");
    try {
      const data = await invoke<LibraryData>("list_library", { query: null });
      if (requestId !== libraryRequestIdRef.current) return;
      setLibrary(data);
      setSearchResult(null);
      setLibraryRevision((current) => current + 1);
      setLibraryStatus(successStatus ?? "媒体库读取完成");
    } catch (error) {
      if (requestId !== libraryRequestIdRef.current) return;
      setLibraryStatus(`媒体库读取失败：${String(error)}`);
    } finally {
      if (requestId === libraryRequestIdRef.current) setIsLoadingLibrary(false);
    }
  }

  async function loadLastScanConfig() {
    const config = await invoke<ScanConfig>("get_last_scan_config");
    setPaths(config.paths);
    setExcludedPaths(config.excludedPaths);
  }

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    if (!isScanning) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [isScanning]);

  useEffect(() => {
    let disposed = false;
    let unlisteners: UnlistenFn[] = [];

    async function initialize() {
      const registered = await Promise.all([
        listen<ScanProgress>("scan-progress", (event) => {
          const progress = event.payload;
          setIsScanning(true);
          setNowMs(Date.now());
          scanStartedAtRef.current = progress.scanStartedAtMs;
          setScanProgress(progress);
          setScanStatus(scanStatusText(progress));
        }),
        listen<ScanSummary>("scan-complete", (event) => {
          const summary = event.payload;
          const completionStatus =
            summary.status === "failed"
              ? `扫描失败，${summary.failedRoots.length} 个根目录均保留了原有索引`
              : summary.status === "partial"
                ? `扫描部分完成，${summary.failedRoots.length} 个根目录保留了原有索引`
                : "扫描完成";
          setLastScan(summary);
          setIsScanning(false);
          setIsStopping(false);
          setSkipListVisible(false);
          setSelectedSkipScanId(null);
          setScanSkips([]);
          setScanHistoryVisible(false);
          setScanHistory([]);
          const completedAtMs = summary.completedAtMs || Date.now();
          const scanStartedAtMs = summary.startedAtMs || scanStartedAtRef.current || completedAtMs;
          setNowMs(completedAtMs);
          setScanProgress({
            phase: "processing",
            discoveredFiles: summary.scannedFiles,
            processedFiles: summary.scannedFiles,
            importedFiles: summary.importedFiles,
            skippedFiles: summary.skippedFiles,
            skippedShortFiles: summary.skippedShortFiles,
            totalFiles: summary.scannedFiles,
            currentPath: null,
            detail: completionStatus,
            scanStartedAtMs,
            currentFileStartedAtMs: null,
            updatedAtMs: completedAtMs,
            ffprobeMissing: summary.ffprobeMissing,
          });
          setScanStatus(completionStatus);
          void refreshLibrary(completionStatus);
          void loadLastScanConfig().catch((error) => setSystemStatus(`扫描配置读取失败：${String(error)}`));
        }),
        listen<ScanFailureEvent>("scan-error", (event) => {
          const failure = event.payload;
          setIsScanning(false);
          setIsStopping(false);
          scanStartedAtRef.current = null;
          setScanStatus(failure.status === "stopped" ? "扫描已停止" : failure.message);
          setScanProgress((current) =>
            current
              ? {
                  ...current,
                  currentPath: null,
                  currentFileStartedAtMs: null,
                  detail: failure.status === "stopped" ? "扫描已停止" : "扫描失败",
                  updatedAtMs: Date.now(),
                }
              : current,
          );
          void refreshLibrary();
        }),
      ]);

      if (disposed) {
        registered.forEach((unlisten) => unlisten());
        return;
      }
      unlisteners = registered;
      setListenersReady(true);
      setSystemStatus("正在检查 ffprobe...");

      void invoke<string>("check_ffprobe")
        .then((message) => setSystemStatus(message))
        .catch((error) => setSystemStatus(`ffprobe 检查失败：${String(error)}`));
      void loadLastScanConfig().catch((error) => setSystemStatus(`扫描配置读取失败：${String(error)}`));
      void refreshLibrary();
    }

    void initialize().catch((error) => {
      setListenersReady(false);
      setSystemStatus(`初始化失败：${String(error)}`);
    });

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    const normalizedQuery = query.trim();
    if (!normalizedQuery || !listenersReady) {
      setSearchResult(null);
      setIsSearching(false);
      return;
    }

    let disposed = false;
    const timer = window.setTimeout(() => {
      setIsSearching(true);
      invoke<LibraryData>("list_library", { query: normalizedQuery })
        .then((data) => {
          if (!disposed) setSearchResult({ query: normalizedQuery, data });
        })
        .catch((error) => {
          if (!disposed) setLibraryStatus(`搜索失败：${String(error)}`);
        })
        .finally(() => {
          if (!disposed) setIsSearching(false);
        });
    }, 250);

    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [query, listenersReady, libraryRevision]);

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
    if (paths.length === 0) {
      setScanStatus("请先选择扫描目录");
      return;
    }
    const selected = await open({
      directory: true,
      multiple: true,
      title: "选择排除子目录",
    });
    if (!selected) return;
    const next = Array.isArray(selected) ? selected : [selected];
    const valid = next.filter((candidate) => paths.some((root) => pathIsStrictlyWithin(candidate, root)));
    setExcludedPaths((current) => Array.from(new Set([...current, ...valid])));
    if (valid.length !== next.length) {
      setScanStatus("部分目录未加入：排除目录必须位于扫描目录内部");
    }
  }

  function removePath(path: string) {
    setPaths((current) => current.filter((item) => item !== path));
    setExcludedPaths((current) => current.filter((item) => !pathIsWithin(item, path)));
  }

  function removeExcludedPath(path: string) {
    setExcludedPaths((current) => current.filter((item) => item !== path));
  }

  async function scan() {
    if (paths.length === 0 || !listenersReady) return;
    const startedAtMs = Date.now();
    setIsScanning(true);
    setIsStopping(false);
    setLastScan(null);
    setSkipListVisible(false);
    setSelectedSkipScanId(null);
    setScanSkips([]);
    setScanHistoryVisible(false);
    setScanHistory([]);
    setNowMs(startedAtMs);
    scanStartedAtRef.current = startedAtMs;
    setScanProgress({
      phase: "discovering",
      discoveredFiles: 0,
      processedFiles: 0,
      importedFiles: 0,
      skippedFiles: 0,
      skippedShortFiles: 0,
      totalFiles: null,
      currentPath: null,
      detail: "准备启动后台扫描",
      scanStartedAtMs: startedAtMs,
      currentFileStartedAtMs: null,
      updatedAtMs: startedAtMs,
      ffprobeMissing: false,
    });
    setScanStatus("后台扫描已启动...");
    try {
      await invoke<void>("start_scan", { paths, excludedPaths });
    } catch (error) {
      setScanStatus(String(error));
      setIsScanning(false);
    }
  }

  async function stopScan() {
    setIsStopping(true);
    setScanStatus("正在停止扫描...");
    try {
      await invoke<void>("stop_scan");
    } catch (error) {
      setIsStopping(false);
      setScanStatus(String(error));
    }
  }

  async function showScanSkips(scanId: number) {
    setIsLoadingSkips(true);
    setSelectedSkipScanId(scanId);
    setScanStatus(`正在读取扫描 #${scanId} 的跳过清单...`);
    try {
      const skips = await invoke<ScanSkip[]>("list_scan_skips", { scanId });
      setScanSkips(skips);
      setSkipListVisible(true);
      setScanStatus(skips.length > 0 ? `已加载 ${skips.length} 个跳过项` : "该次扫描没有非短视频跳过项");
    } catch (error) {
      setScanStatus(String(error));
    } finally {
      setIsLoadingSkips(false);
    }
  }

  async function showScanHistory() {
    setIsLoadingHistory(true);
    setScanStatus("正在读取扫描历史...");
    try {
      const runs = await invoke<ScanRun[]>("list_scan_history");
      setScanHistory(runs);
      setScanHistoryVisible(true);
      setScanStatus(runs.length > 0 ? `已加载 ${runs.length} 条扫描历史` : "没有扫描历史记录");
    } catch (error) {
      setScanStatus(String(error));
    } finally {
      setIsLoadingHistory(false);
    }
  }

  async function applyMerge(kind: MergeKind, group: MediaGroup, targetName: string) {
    setLibraryStatus(targetName.trim() ? "正在保存分类规则..." : "正在清除分类规则...");
    try {
      const data = await invoke<LibraryData>("set_merge_rules", {
        request: {
          kind,
          sourceKeys: group.sourceKeys,
          targetName,
        },
      });
      setLibrary(data);
      setSearchResult(null);
      setLibraryRevision((current) => current + 1);
      setLibraryStatus(targetName.trim() ? "分类规则已保存" : "分类规则已清除");
    } catch (error) {
      setLibraryStatus(String(error));
    }
  }

  const normalizedQuery = query.trim();
  const matchedSearchResult = searchResult?.query === normalizedQuery ? searchResult.data : null;
  const visibleLibrary = matchedSearchResult ?? library;
  const useLocalSummaryFilter = normalizedQuery.length > 0 && matchedSearchResult == null;
  const musicDirectories = useMemo(
    () =>
      useLocalSummaryFilter
        ? visibleLibrary.musicDirectories.filter((directory) => directoryMatches(directory, query))
        : visibleLibrary.musicDirectories,
    [visibleLibrary, query, useLocalSummaryFilter],
  );
  const videoDirectories = useMemo(
    () =>
      useLocalSummaryFilter
        ? visibleLibrary.videoDirectories.filter((directory) => directoryMatches(directory, query))
        : visibleLibrary.videoDirectories,
    [visibleLibrary, query, useLocalSummaryFilter],
  );
  const musicArtists = useMemo(
    () =>
      useLocalSummaryFilter
        ? visibleLibrary.musicArtists.filter((group) => groupMatches(group, query))
        : visibleLibrary.musicArtists,
    [visibleLibrary, query, useLocalSummaryFilter],
  );
  const videoSeries = useMemo(
    () =>
      useLocalSummaryFilter
        ? visibleLibrary.videoSeries.filter((group) => groupMatches(group, query))
        : visibleLibrary.videoSeries,
    [visibleLibrary, query, useLocalSummaryFilter],
  );

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
  const scanElapsedMs = scanProgress ? nowMs - scanProgress.scanStartedAtMs : null;
  const currentFileElapsedMs = scanProgress?.currentFileStartedAtMs ? nowMs - scanProgress.currentFileStartedAtMs : null;
  const lastProgressAgeMs = scanProgress ? nowMs - scanProgress.updatedAtMs : null;
  const activeBrowseMode = activeEntry === "music" ? musicBrowseMode : videoBrowseMode;
  const activeTitle =
    activeEntry === "music"
      ? musicBrowseMode === "directory"
        ? "音乐目录"
        : "音乐作者"
      : videoBrowseMode === "directory"
        ? "影视目录"
        : "影视系列";
  const activeEmptyText = isLoadingLibrary ? "正在读取媒体库..." : normalizedQuery ? "没有匹配的资源。" : activeEntry === "music" ? "暂无音乐资源。" : "暂无影视资源。";

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
              <div className="path-chip removable" key={path} title={path}>
                <span>{path}</span>
                <button type="button" disabled={isScanning} onClick={() => removePath(path)}>
                  移除
                </button>
              </div>
            ))}
          </div>
          <div className="button-row">
            <button type="button" disabled={isScanning} onClick={chooseDirectories}>
              选择目录
            </button>
            {isScanning ? (
              <button type="button" className="danger" disabled={isStopping} onClick={stopScan}>
                停止
              </button>
            ) : (
              <button type="button" className="primary" disabled={paths.length === 0 || !listenersReady} onClick={scan}>
                扫描
              </button>
            )}
          </div>

          <div className="exclude-header">
            <span>排除子目录</span>
            <button type="button" disabled={isScanning} onClick={chooseExcludedDirectories}>
              添加排除
            </button>
          </div>
          <div className="path-list compact">
            {excludedPaths.length === 0 ? <span className="muted">未设置排除目录</span> : null}
            {excludedPaths.map((path) => (
              <div className="path-chip removable" key={path} title={path}>
                <span>{path}</span>
                <button type="button" disabled={isScanning} onClick={() => removeExcludedPath(path)}>
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
          <div className="status-list">
            <div>
              <span>系统</span>
              <strong>{systemStatus}</strong>
            </div>
            <div>
              <span>扫描</span>
              <strong>{scanStatus}</strong>
            </div>
            <div>
              <span>媒体库</span>
              <strong>{libraryStatus}</strong>
            </div>
          </div>
          {isLoadingLibrary ? <p className="muted">媒体库正在后台读取...</p> : null}
          {isSearching ? <p className="muted">正在后台搜索文件路径...</p> : null}
          {scanProgress ? (
            <div className="progress-block">
              <div className="progress-bar" data-indeterminate={percent == null && isScanning ? "true" : "false"}>
                <div className="progress-fill" style={{ width: `${percent ?? 42}%` }} />
              </div>
              <div className="progress-meta">
                <span>发现 {scanProgress.discoveredFiles}</span>
                <span>已处理 {scanProgress.processedFiles}</span>
                <span>已入库 {scanProgress.importedFiles}</span>
                <span>已跳过 {scanProgress.skippedFiles}</span>
                <span>短视频过滤 {scanProgress.skippedShortFiles}</span>
              </div>
              <div className="progress-detail">
                <div>
                  <span>阶段</span>
                  <strong>{phaseLabel(scanProgress.phase)}</strong>
                </div>
                <div>
                  <span>当前动作</span>
                  <strong>{scanProgress.detail}</strong>
                </div>
                <div>
                  <span>当前文件耗时</span>
                  <strong>{formatElapsedMs(currentFileElapsedMs)}</strong>
                </div>
                <div>
                  <span>总耗时</span>
                  <strong>{formatElapsedMs(scanElapsedMs)}</strong>
                </div>
                <div>
                  <span>最近更新</span>
                  <strong>{formatElapsedMs(lastProgressAgeMs)}前 · {formatClock(scanProgress.updatedAtMs)}</strong>
                </div>
                <div>
                  <span>处理速度</span>
                  <strong>{processingSpeed(scanProgress, nowMs)}</strong>
                </div>
                <div>
                  <span>ffprobe</span>
                  <strong>{scanProgress.ffprobeMissing ? "未找到" : "可用"}</strong>
                </div>
              </div>
              {scanProgress.currentPath ? (
                <div className="current-file">
                  <span>当前路径</span>
                  <code title={scanProgress.currentPath}>{scanProgress.currentPath}</code>
                </div>
              ) : null}
            </div>
          ) : null}
          {lastScan ? (
            <div className="last-scan-block">
              <p className="muted">
                最近扫描：{scanRunStatusLabel(lastScan.status)} · 入库 {lastScan.importedFiles} 个媒体文件，记录 {lastScan.recordedDirectories} 个目录，短视频过滤 {lastScan.skippedShortFiles} 个，用时 {formatElapsedMs(lastScan.durationMs)}
              </p>
              {lastScan.failedRoots.map((root) => (
                <p className="root-failure" key={root.path}>
                  <code title={root.path}>{root.path}</code>
                  <span>{root.detail}</span>
                </p>
              ))}
              <button type="button" className="skip-list-toggle" disabled={isLoadingSkips || isScanning} onClick={() => void showScanSkips(lastScan.scanId)}>
                {isLoadingSkips ? "读取中" : "查看跳过清单"}
              </button>
            </div>
          ) : null}
          <div className="side-action-row">
            <button
              type="button"
              className="skip-list-toggle"
              disabled={isLoadingHistory || isScanning}
              onClick={() => (scanHistoryVisible ? setScanHistoryVisible(false) : void showScanHistory())}
            >
              {isLoadingHistory ? "读取中" : scanHistoryVisible ? "收起扫描历史" : "查看扫描历史"}
            </button>
          </div>
          {skipListVisible ? (
            <div className="skip-list">
              <div className="skip-list-header">
                <strong>扫描 #{selectedSkipScanId} 跳过清单</strong>
                <button type="button" onClick={() => setSkipListVisible(false)}>
                  关闭
                </button>
              </div>
              <span className="muted">不包含短视频过滤项</span>
              {scanSkips.length === 0 ? <p className="muted">没有非短视频跳过项。</p> : null}
              {scanSkips.map((item) => (
                <div className="skip-item" key={item.id}>
                  <div>
                    <strong>{item.fileName}</strong>
                    <span>{skipReasonLabel(item.reason)} · {item.detail}</span>
                  </div>
                  <code title={item.path}>{item.path}</code>
                </div>
              ))}
            </div>
          ) : null}
          {scanHistoryVisible ? (
            <div className="scan-history">
              <div className="scan-history-header">
                <strong>扫描历史</strong>
                <span>最近 {scanHistory.length} 条</span>
              </div>
              {scanHistory.length === 0 ? <p className="muted">没有扫描历史记录。</p> : null}
              {scanHistory.map((run) => (
                <div className="scan-run" key={run.id}>
                  <div>
                    <strong>
                      #{run.id} · {scanRunStatusLabel(run.status)} · {formatDateTime(run.completedAtMs || run.startedAtMs)}
                    </strong>
                    <span>
                      入库 {run.importedFiles} · 跳过 {run.skippedFiles} · 短视频 {run.skippedShortFiles} · 目录 {run.recordedDirectories} · 用时 {formatElapsedMs(run.durationMs)} · ffprobe {run.ffprobeMissing ? "未找到" : "可用"}
                    </span>
                  </div>
                  {run.errorMessage ? <span className="run-error">{run.errorMessage}</span> : null}
                  {run.failedRoots.map((root) => (
                    <div className="root-failure" key={root.path}>
                      <code title={root.path}>{root.path}</code>
                      <span>{root.detail}</span>
                    </div>
                  ))}
                  <code title={run.paths.join("\n")}>扫描目录：{run.paths.join(" | ") || "-"}</code>
                  {run.excludedPaths.length > 0 ? <code title={run.excludedPaths.join("\n")}>排除目录：{run.excludedPaths.join(" | ")}</code> : null}
                  {run.skippedFiles > run.skippedShortFiles ? (
                    <button type="button" disabled={isLoadingSkips} onClick={() => void showScanSkips(run.id)}>
                      查看本次跳过明细
                    </button>
                  ) : null}
                </div>
              ))}
            </div>
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
          <GroupSection
            title="音乐作者"
            emptyText={activeEmptyText}
            groups={musicArtists}
            mode={viewMode}
            mergeKind="music_artist"
            disabled={isScanning}
            revision={libraryRevision}
            onMerge={applyMerge}
          />
        ) : activeEntry === "video" && videoBrowseMode === "series" ? (
          <GroupSection
            title="影视系列"
            emptyText={activeEmptyText}
            groups={videoSeries}
            mode={viewMode}
            mergeKind="video_series"
            disabled={isScanning}
            revision={libraryRevision}
            onMerge={applyMerge}
          />
        ) : (
          <DirectorySection
            title={activeTitle}
            emptyText={activeEmptyText}
            directories={activeEntry === "music" ? musicDirectories : videoDirectories}
            mode={viewMode}
            revision={libraryRevision}
          />
        )}
      </section>
    </main>
  );
}
