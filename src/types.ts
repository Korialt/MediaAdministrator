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
  mediaKind: "music" | "video";
  fileCount: number;
  totalSize: number;
};

export type MediaGroup = {
  key: string;
  name: string;
  subtitle: string | null;
  familyName: string | null;
  fileCount: number;
  totalSize: number;
  sourceKeys: string[];
  resourceKeys: string[];
  childGroups: MediaGroup[];
};

export type ResourcePage = {
  files: ResourceVariant[];
  total: number;
  offset: number;
  limit: number;
};

export type FailedRoot = {
  path: string;
  detail: string;
};

export type LibraryData = {
  musicDirectories: MediaDirectory[];
  videoDirectories: MediaDirectory[];
  musicArtists: MediaGroup[];
  videoSeries: MediaGroup[];
};

export type ScanSummary = {
  scanId: number;
  startedAtMs: number;
  completedAtMs: number;
  durationMs: number;
  scannedFiles: number;
  importedFiles: number;
  skippedFiles: number;
  skippedShortFiles: number;
  recordedDirectories: number;
  ffprobeMissing: boolean;
  status: ScanRunStatus;
  failedRoots: FailedRoot[];
};

export type ScanRunStatus = "running" | "completed" | "partial" | "stopped" | "failed";

export type ScanRun = {
  id: number;
  startedAtMs: number;
  completedAtMs: number;
  durationMs: number;
  scannedFiles: number;
  importedFiles: number;
  skippedFiles: number;
  skippedShortFiles: number;
  recordedDirectories: number;
  ffprobeMissing: boolean;
  status: ScanRunStatus;
  errorMessage: string | null;
  failedRoots: FailedRoot[];
  paths: string[];
  excludedPaths: string[];
  createdAt: string;
};

export type ScanConfig = {
  paths: string[];
  excludedPaths: string[];
};

export type ScanSkip = {
  id: number;
  scanId: number | null;
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
  ffprobeMissing: boolean;
};

export type ScanFailureEvent = {
  scanId: number | null;
  status: "stopped" | "failed";
  message: string;
};
