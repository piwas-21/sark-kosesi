// Artifact uploads reject URI punctuation such as '?' even on Linux filesystems.
export function lighthouseEvidenceName(path, run) {
  return `lighthouse-${path.replace(/[^A-Za-z0-9_-]/g, "_")}-${run}.json`;
}
