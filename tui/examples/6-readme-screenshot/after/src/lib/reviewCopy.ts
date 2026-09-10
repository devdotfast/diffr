export function reviewButtonLabel(fileCount: number) {
  return fileCount === 1 ? "Review 1 file" : `Review ${fileCount} files`;
}

export function reviewTimestampLabel(lastUpdated: string) {
  return `Updated ${lastUpdated}`;
}
