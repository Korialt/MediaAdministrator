export type ResourceVariant = {
  id: number;
  path: string;
  fileName: string;
  rootPath: string;
  fileSize: number;
  durationSeconds: number | null;
  container: string | null;
  videoCodec: string | null;
  audioCodec: string | null;
  width: number | null;
  height: number | null;
  resolution: string | null;
  source: string | null;
  releaseGroup: string | null;
  seasonNumber: number | null;
  episodeNumber: number | null;
  titleGuess: string;
  mediaKind: "music" | "video";
  musicArtist: string | null;
  musicAlbum: string | null;
  musicTitle: string | null;
  musicArtistSource: string | null;
  seriesTitle: string | null;
  seriesSource: string | null;
};

export type MediaDirectory = {
  key: string;
  path: string;
  name: string;
  relativePath: string;
  parentName: string | null;
  fileCount: number;
  totalSize: number;
  files: ResourceVariant[];
};

export type MediaGroup = {
  key: string;
  name: string;
  subtitle: string | null;
  fileCount: number;
  totalSize: number;
  sourceKeys: string[];
  files: ResourceVariant[];
  childGroups: MediaGroup[];
};

export type LibraryData = {
  musicDirectories: MediaDirectory[];
  videoDirectories: MediaDirectory[];
  musicArtists: MediaGroup[];
  videoSeries: MediaGroup[];
};

export type ScanSummary = {
  scannedFiles: number;
  importedFiles: number;
  skippedFiles: number;
  skippedShortFiles: number;
  recordedDirectories: number;
  ffprobeMissing: boolean;
  library: LibraryData;
};

export type ScanSkip = {
  id: number;
  path: string;
  fileName: string;
  rootPath: string;
  reason: string;
  detail: string;
  isShortVideo: boolean;
  fileSize: number | null;
  modifiedMs: number | null;
  createdAt: string;
};

export type ScanProgress = {
  phase: "discovering" | "processing";
  discoveredFiles: number;
  processedFiles: number;
  importedFiles: number;
  skippedFiles: number;
  skippedShortFiles: number;
  totalFiles: number | null;
  currentPath: string | null;
  detail: string;
  scanStartedAtMs: number;
  currentFileStartedAtMs: number | null;
  updatedAtMs: number;
  canSkipCurrentFile: boolean;
  ffprobeMissing: boolean;
};
