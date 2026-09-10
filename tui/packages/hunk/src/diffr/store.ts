/** Hold streamed files separately from presentation state and notify React in batches. */
import type { DiffEvent, DiffFile } from "./wire";
export interface Snapshot {
  files: DiffFile[];
  errors: string[];
  total: number;
  complete: boolean;
}
export class DiffStore {
  private value: Snapshot = {
    files: [],
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
      this.value = { ...this.value, total: event.total };
    if (event.type === "file")
      this.value = { ...this.value, files: [...this.value.files, event] };
    if (event.type === "file_error")
      this.value = {
        ...this.value,
        errors: [
          ...this.value.errors,
          `${event.file.new_path ?? event.file.old_path}: ${event.message}`,
        ],
      };
    if (event.type === "complete")
      this.value = { ...this.value, complete: true };
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
