/** Hold streamed files separately from presentation state and notify React in batches. */
import { fileIdentity, filePath, type FileChange, type DiffEvent, type DiffFile } from "./wire";
export interface Snapshot {
  files: DiffFile[];
  inventory: FileChange[];
  failedFiles: Map<string, string>;
  errors: string[];
  total: number;
  complete: boolean;
}
export class DiffStore {
  private value: Snapshot = {
    files: [],
    inventory: [],
    failedFiles: new Map(),
    errors: [],
    total: 0,
    complete: false,
  };
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
      this.value = { ...this.value, total: event.files.length, inventory: event.files };
    if (event.type === "file") {
      const identity = fileIdentity(event.file);
      const inventory = this.value.inventory.some((f) => fileIdentity(f.file) === identity)
        ? this.value.inventory
        : [
            ...this.value.inventory,
            { file: event.file, status: "modified" as const, visibility: { collapsed: false, label: "" } },
          ];
      if (event.diff)
        this.value = { ...this.value, inventory, files: [...this.value.files, { ...event, diff: event.diff }] };
      else if (event.error)
        this.value = {
          ...this.value,
          inventory,
          failedFiles: new Map(this.value.failedFiles).set(identity, event.error.message),
          errors: [...this.value.errors, `${filePath(event.file)}: ${event.error.message}`],
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
    this.listeners.forEach((listener) => listener());
  }
  fail(error: unknown) {
    this.value = {
      ...this.value,
      complete: true,
      errors: [...this.value.errors, String(error)],
    };
    this.listeners.forEach((listener) => listener());
  }
}
