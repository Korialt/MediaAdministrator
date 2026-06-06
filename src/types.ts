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
};

export type LibraryGroup = {
  key: string;
  title: string;
  seasonNumber: number | null;
  episodeNumber: number | null;
  variants: ResourceVariant[];
};

export type ScanSummary = {
  scannedFiles: number;
  importedFiles: number;
  skippedFiles: number;
  ffprobeMissing: boolean;
  groups: LibraryGroup[];
};
