import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import type { LibraryGroup, ScanSummary } from "./types";

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

function episodeLabel(group: LibraryGroup): string {
  if (group.seasonNumber == null || group.episodeNumber == null) return "单项资源";
  return `S${group.seasonNumber.toString().padStart(2, "0")}E${group.episodeNumber
    .toString()
    .padStart(2, "0")}`;
}

export default function App() {
  const [paths, setPaths] = useState<string[]>([]);
  const [library, setLibrary] = useState<LibraryGroup[]>([]);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("正在检查 ffprobe...");
  const [isScanning, setIsScanning] = useState(false);
  const [lastScan, setLastScan] = useState<ScanSummary | null>(null);

  async function refreshLibrary() {
    const groups = await invoke<LibraryGroup[]>("list_library");
    setLibrary(groups);
  }

  useEffect(() => {
    invoke<string>("check_ffprobe")
      .then((message) => setStatus(message))
      .catch((error) => setStatus(String(error)));
    refreshLibrary().catch((error) => setStatus(String(error)));
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
    setStatus("正在扫描目录...");
    try {
      const summary = await invoke<ScanSummary>("scan_paths", { paths });
      setLastScan(summary);
      setLibrary(summary.groups);
      setStatus(summary.ffprobeMissing ? "扫描完成，但没有找到 ffprobe" : "扫描完成");
    } catch (error) {
      setStatus(String(error));
    } finally {
      setIsScanning(false);
    }
  }

  const filteredLibrary = useMemo(() => {
    const text = query.trim().toLowerCase();
    if (!text) return library;
    return library.filter((group) => {
      if (group.title.toLowerCase().includes(text)) return true;
      return group.variants.some((variant) => variant.path.toLowerCase().includes(text));
    });
  }, [library, query]);

  const totals = useMemo(() => {
    const variants = library.reduce((sum, group) => sum + group.variants.length, 0);
    const bytes = library.reduce(
      (sum, group) => sum + group.variants.reduce((inner, variant) => inner + variant.fileSize, 0),
      0,
    );
    return { variants, bytes };
  }, [library]);

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div>
            <h1>Media Administrator</h1>
            <p>本地媒体资源索引</p>
          </div>
        </div>

        <section className="panel">
          <div className="panel-title">目录</div>
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
            <span>{library.length}</span>
            <small>条目</small>
          </div>
          <div>
            <span>{totals.variants}</span>
            <small>资源</small>
          </div>
          <div>
            <span>{formatBytes(totals.bytes)}</span>
            <small>容量</small>
          </div>
        </section>

        <section className="panel">
          <div className="panel-title">状态</div>
          <p className="status">{status}</p>
          {lastScan ? (
            <p className="muted">
              最近扫描：{lastScan.importedFiles} 个媒体文件，跳过 {lastScan.skippedFiles} 个文件
            </p>
          ) : null}
        </section>
      </aside>

      <section className="content">
        <header className="toolbar">
          <div>
            <h2>媒体库</h2>
            <p>每个资源路径都会保留在对应条目下。</p>
          </div>
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索标题或路径"
            aria-label="搜索标题或路径"
          />
        </header>

        <div className="library-list">
          {filteredLibrary.length === 0 ? (
            <div className="empty">暂无资源。选择目录后开始扫描。</div>
          ) : null}

          {filteredLibrary.map((group) => (
            <details className="media-group" key={group.key}>
              <summary>
                <div>
                  <strong>{group.title}</strong>
                  <span>{episodeLabel(group)}</span>
                </div>
                <b>{group.variants.length} 个资源</b>
              </summary>

              <div className="variant-table-wrap">
                <table className="variant-table">
                  <thead>
                    <tr>
                      <th>版本</th>
                      <th>规格</th>
                      <th>大小</th>
                      <th>时长</th>
                      <th>路径</th>
                    </tr>
                  </thead>
                  <tbody>
                    {group.variants.map((variant) => (
                      <tr key={variant.id}>
                        <td>
                          <div className="variant-name">{variant.releaseGroup ?? "未知发布组"}</div>
                          <div className="muted">{variant.source ?? "未知来源"}</div>
                        </td>
                        <td>
                          <div>
                            {[variant.resolution, variant.videoCodec, variant.audioCodec].filter(Boolean).join(" / ") ||
                              "-"}
                          </div>
                          <div className="muted">{variant.container ?? "-"}</div>
                        </td>
                        <td>{formatBytes(variant.fileSize)}</td>
                        <td>{formatDuration(variant.durationSeconds)}</td>
                        <td>
                          <code title={variant.path}>{variant.path}</code>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </details>
          ))}
        </div>
      </section>
    </main>
  );
}
