# Media Administrator

面向 Windows 的本地媒体资源管理器，使用 Tauri、React、TypeScript、Rust 和 SQLite 构建。软件保留每个物理文件及其所在目录，不对 BDRip、WEBRip、发布组或文件大小进行评分和删减。

## 主要功能

- 音乐与影视使用独立入口、目录和分类视图。
- 扫描多个根目录，并排除指定子目录及其全部内容。
- 通过媒体流判断资源类型；只有音频流的文件进入音乐库，包含有效视频流的文件进入影视库。
- 过滤时长少于 5 分钟的视频，并在扫描历史中单独统计。
- 后台扫描、实时进度、当前文件耗时、停止扫描和按扫描批次保存的跳过清单。
- 按目录汇总资源，展开目录或分类时分页读取文件，避免启动时传输全部文件明细。
- 音乐作者、影视作品和作品族分类，支持手动重命名与合并。
- 列表/图表布局切换与暗黑模式。

## 识别规则

音乐作者按以下优先级识别：

1. 文件媒体标签中的 `album_artist`、`artist` 或 `composer`。
2. 明确的 `作者 - 标题` 文件名。
3. `作者/专辑/文件` 目录层级。
4. `未知作者`。

影视作品标题按以下优先级识别：

1. 含明确作品名和集号的文件名，例如 `[Group][Title][21]`、`Title - 01`、`S01E02`、`1x02`。
2. 文件所在位置向上最近的有效目录名。
3. 当前扫描根目录的名称，适用于直接扫描作品目录且文件名为 `01.mkv` 的情况。
4. 无法可靠识别时归入 `未识别系列`。

目录标题会清理发布组、分辨率、编码、来源、季数、`Part`、`Cour` 等后缀。纯数字集号不会被当成作品名；`86 Eighty-Six`、`1917` 等合法数字标题会保留。作品族采用保守规则识别，支持 Monogatari 命名和显式父子目录前缀，也可在界面中手动设置。

## 运行依赖

- Windows 10/11 与 WebView2 Runtime
- `ffprobe` 已安装并可从当前用户的 `PATH` 调用
- 源码构建需要 Node.js 24 和 Rust stable

发布包不包含 ffmpeg/ffprobe。可在普通终端执行 `ffprobe -version` 验证当前用户环境。

## 数据位置

数据库固定写入程序可执行文件所在目录：

```text
media_administrator.sqlite3
media_administrator.sqlite3-wal
media_administrator.sqlite3-shm
```

程序目录必须允许当前 Windows 用户写入。绿色版应解压到普通用户目录；数据库需要随程序目录一起备份。首次运行新版且当前目录没有数据库时，软件会从旧版 AppData 位置通过 SQLite 在线备份迁移现有数据。

## 本地开发

```bash
npm ci
npm run check:versions
npm run typecheck
cargo test --manifest-path src-tauri/Cargo.toml --locked
npm run dev
```

本地构建：

```bash
npm run build -- --target x86_64-pc-windows-msvc
```

## Windows 发布

`.github/workflows/windows-build.yml` 使用 `windows-2025-vs2026`、Node.js 24、Rust stable、npm 下载缓存和 `Swatinem/rust-cache`。CI 会先检查所有清单版本一致性，再执行前端类型检查、Rust 测试和 Tauri 构建。

推送 `v*` 标签会执行一次 Windows 构建，并在测试通过后创建 GitHub Release，附带：

- Windows x64 绿色版 ZIP
- Windows x64 NSIS 安装程序

`main` 分支普通推送不会触发发布构建；Pull Request 与手动运行仅用于验证，手动运行可显式选择发布。标签必须严格等于 `v<package.json version>`。
