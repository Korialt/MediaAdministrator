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

export type LibraryData = {
  musicDirectories: MediaDirectory[];
  videoDirectories: MediaDirectory[];
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

export type ScanProgress = {
  phase: "discovering" | "processing";
  discoveredFiles: number;
  processedFiles: number;
  importedFiles: number;
  skippedFiles: number;
  skippedShortFiles: number;
  totalFiles: number | null;
  currentPath: string | null;
  ffprobeMissing: boolean;
};
