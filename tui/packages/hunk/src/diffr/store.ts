/** Hold streamed files separately from presentation state and notify React in batches. */
import { fileIdentity, filePath, type FileChange, type DiffEvent, type DiffFile, type Region, type Source } from "./wire";
type StartEvent = Extract<DiffEvent, { type: "start" }>;
export interface Snapshot {
  /** The two ends of the comparison, from the start event. */
  comparison: { lhs: StartEvent["lhs"]; rhs: StartEvent["rhs"] } | null;
  files: DiffFile[];
  inventory: FileChange[];
  failedFiles: Map<string, string>;
  errors: string[];
  total: number;
  complete: boolean;
}
export class DiffStore {
  private value: Snapshot = {
    comparison: null,
    files: [],
    inventory: [],
    failedFiles: new Map(),
    errors: [],
    total: 0,
    complete: false,
  };
  private notification: ReturnType<typeof setTimeout> | undefined;
  private notify() {
    if (this.notification !== undefined || this.listeners.size === 0) return;
    // Stream reads can deliver many records in a single event-loop turn. Let input
    // and painting run, and rebuild the viewer at most once per frame-sized batch.
    this.notification = setTimeout(() => {
      this.notification = undefined;
      this.listeners.forEach((listener) => listener());
    }, 16);
  }
  private listeners = new Set<() => void>();
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  getSnapshot = () => this.value;
  accept(event: DiffEvent) {
    if (event.type === "start")
      this.value = { ...this.value, comparison: { lhs: event.lhs, rhs: event.rhs }, total: event.files.length, inventory: event.files };
    if (event.type === "file") {
      const identity = fileIdentity(event.file);
      if (event.diff)
        this.value = { ...this.value, files: [...this.value.files, { ...event, diff: event.diff }] };
      else if (event.error)
        this.value = {
          ...this.value,
          failedFiles: new Map(this.value.failedFiles).set(identity, event.error.message),
          errors: [...this.value.errors, `${filePath(event.file)}: ${event.error.message}`],
        };
    }
    if (event.type === "annotations") {
      const identity = fileIdentity(event.file);
      const labels = new Map(event.annotations.map(({ region_id, label }) => [region_id, label]));
      const updateRegion = (region: Region): Region => ({
        ...region,
        visibility: labels.has(region.id)
          ? { ...region.visibility, label: labels.get(region.id)! }
          : region.visibility,
        children: region.children.map(updateRegion),
      });
      const updateSource = (source: Source | undefined) => source && ({
        ...source, regions: source.regions.map(updateRegion),
      });
      this.value = {
        ...this.value,
        files: this.value.files.map(file => fileIdentity(file.file) === identity && file.diff.type === "text" && labels.size
          ? { ...file, diff: { ...file.diff, lhs: updateSource(file.diff.lhs), rhs: updateSource(file.diff.rhs) } }
          : file),
        errors: event.error
          ? [...this.value.errors, `${filePath(event.file)}: summaries unavailable: ${event.error.message}`]
          : this.value.errors,
      };
    }
    if (event.type === "complete")
      this.value = {
        ...this.value,
        complete: true,
        errors: event.aborted
          ? [...this.value.errors, `diffr stopped early: ${event.aborted.message}`]
          : this.value.errors,
      };
    this.notify();
  }
  fail(error: unknown) {
    this.value = {
      ...this.value,
      complete: true,
      errors: [...this.value.errors, String(error)],
    };
    this.notify();
  }
}
