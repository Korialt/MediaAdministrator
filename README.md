# Media Administrator

Windows-first desktop media library manager built with Tauri, React, TypeScript and SQLite.

The app keeps every physical resource as a separate variant. It does not score or discard versions. A movie, episode, concert or album entry can show files from different folders, release groups and sources.

## Requirements

- Node.js 20 or newer
- Rust stable toolchain
- ffmpeg/ffprobe installed and available from `PATH`

## Development

```bash
npm install
npm run dev
```

## Build

```bash
npm run build
```

Windows installers are built by `.github/workflows/windows-build.yml` on the `windows-latest` GitHub Actions runner.

## Current Scope

- Select one or more directories.
- Recursively scan common video and audio files.
- Call `ffprobe` from `PATH` to read duration, container, codecs and resolution.
- Parse filename hints such as release group, source, `S01E02`, `1x02` and anime-style `Title - 02`.
- Store resources in a local SQLite database.
- Display all variants grouped by logical title/episode.
