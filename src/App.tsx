import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import type { LibraryData, LibraryModule, MediaDirectory, ResourceVariant, ScanProgress, ScanSummary } from "./types";

const EMPTY_LIBRARY: LibraryData = { modules: [] };

type Theme = "light" | "dark";

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

function moduleMatches(module: LibraryModule, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  if (module.title.toLowerCase().includes(text)) return true;
  return module.directories.some((directory) => {
    if (directory.name.toLowerCase().includes(text) || directory.path.toLowerCase().includes(text)) return true;
    return directory.files.some((file) => file.fileName.toLowerCase().includes(text) || file.path.toLowerCase().includes(text));
  });
}

function fileSpec(file: ResourceVariant): string {
  const parts = [file.resolution, file.videoCodec, file.audioCodec].filter(Boolean);
  return parts.length > 0 ? parts.join(" / ") : file.container ?? "-";
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
                <div className="muted">
                  {[episodeLabel(file), file.releaseGroup, file.source].filter(Boolean).join(" / ") || file.titleGuess}
                </div>
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

function DirectoryBlock({ directory }: { directory: MediaDirectory }) {
  return (
    <details className="directory-block">
      <summary>
        <div className="directory-main">
          <strong>{directory.name}</strong>
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

function ModuleList({ title, emptyText, modules }: { title: string; emptyText: string; modules: LibraryModule[] }) {
  return (
    <section className="library-section">
      <header className="section-header">
        <h3>{title}</h3>
        <span>{modules.length} 个模块</span>
      </header>

      {modules.length === 0 ? <div className="empty">{emptyText}</div> : null}

      <div className="module-list">
        {modules.map((module) => (
          <details className="module-block" key={module.key}>
            <summary>
              <div>
                <strong>{module.title}</strong>
                <span>{module.kind === "music" ? "音乐作者" : "影视模块"}</span>
              </div>
              <b>
                {module.directoryCount} 个目录 · {module.fileCount} 个文件 · {formatBytes(module.totalSize)}
              </b>
            </summary>

            <div className="directory-list">
              {module.directories.map((directory) => (
                <DirectoryBlock directory={directory} key={directory.key} />
              ))}
            </div>
          </details>
        ))}
      </div>
    </section>
  );
}

export default function App() {
  const [paths, setPaths] = useState<string[]>([]);
  const [library, setLibrary] = useState<LibraryData>(EMPTY_LIBRARY);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("正在检查 ffprobe...");
  const [isScanning, setIsScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [lastScan, setLastScan] = useState<ScanSummary | null>(null);
  const [theme, setTheme] = useState<Theme>(() => (window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light"));

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
      setStatus(String(event.payload));
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

  async function scan() {
    if (paths.length === 0) return;
    setIsScanning(true);
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
      await invoke<void>("start_scan", { paths });
    } catch (error) {
      setStatus(String(error));
      setIsScanning(false);
    }
  }

  const filteredModules = useMemo(() => library.modules.filter((module) => moduleMatches(module, query)), [library, query]);
  const musicModules = useMemo(() => filteredModules.filter((module) => module.kind === "music"), [filteredModules]);
  const videoModules = useMemo(() => filteredModules.filter((module) => module.kind === "video"), [filteredModules]);

  const totals = useMemo(() => {
    const directories = library.modules.reduce((sum, module) => sum + module.directoryCount, 0);
    const files = library.modules.reduce((sum, module) => sum + module.fileCount, 0);
    const bytes = library.modules.reduce((sum, module) => sum + module.totalSize, 0);
    return { modules: library.modules.length, directories, files, bytes };
  }, [library]);

  const percent = progressPercent(scanProgress);

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div>
            <h1>Media Administrator</h1>
            <p>目录级媒体资源索引</p>
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
            <button type="button" className="primary" disabled={paths.length === 0 || isScanning} onClick={scan}>
              {isScanning ? "扫描中" : "扫描"}
            </button>
          </div>
        </section>

        <section className="stats">
          <div>
            <span>{totals.modules}</span>
            <small>模块</small>
          </div>
          <div>
            <span>{totals.directories}</span>
            <small>目录</small>
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
            <h2>媒体库</h2>
            <p>按作者、影视系列和电影模块归类；展开模块查看包含它的所有目录。</p>
          </div>
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索模块、目录或文件路径"
            aria-label="搜索模块、目录或文件路径"
          />
        </header>

        <ModuleList title="音乐作者" emptyText="暂无音乐作者。" modules={musicModules} />
        <ModuleList title="影视 / 番剧 / 电影" emptyText="暂无影视模块。" modules={videoModules} />
      </section>
    </main>
  );
}
