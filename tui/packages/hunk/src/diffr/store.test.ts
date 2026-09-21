import { expect, test } from "bun:test";
import { DiffStore } from "./store";
import { createTestDiffFile, startFor } from "./fixture";

test("a burst of streamed files publishes one complete snapshot", async () => {
  const store = new DiffStore(), file = createTestDiffFile();
  const snapshots: ReturnType<DiffStore["getSnapshot"]>[] = [];
  const published = new Promise<void>(resolve => {
    store.subscribe(() => { snapshots.push(store.getSnapshot()); resolve(); });
  });
  const initial = store.getSnapshot();
  store.accept(startFor([file]));
  for (let i = 0; i < 226; i++) store.accept(file);
  store.accept({ type: "complete", succeeded: 226, failed: 0 });
  expect(snapshots).toHaveLength(0);
  expect(initial.files).toHaveLength(0);
  await published;
  expect(snapshots).toHaveLength(1);
  expect(snapshots[0].files).toHaveLength(226);
  expect(snapshots[0].complete).toBe(true);

  const failed = new Promise<void>(resolve => {
    const unsubscribe = store.subscribe(() => { unsubscribe(); resolve(); });
  });
  store.fail("subprocess failed");
  await failed;
  expect(snapshots).toHaveLength(2);
  expect(snapshots[1].errors).toEqual(["subprocess failed"]);
  expect(snapshots[0].errors).toEqual([]);
});

test("an unsubscribed viewer is not notified by a pending batch", async () => {
  const store = new DiffStore();
  let calls = 0;
  const unsubscribe = store.subscribe(() => calls++);
  store.fail("stopped");
  unsubscribe();
  await Bun.sleep(30);
  expect(calls).toBe(0);
});

test("deferred labels preserve initial files, counts, and collapse defaults", () => {
  const store = new DiffStore(), file = createTestDiffFile();
  const start = startFor([file]);
  if (start.type !== "start") throw Error("Expected start");
  store.accept({ ...start, version: 4 });
  store.accept(file);
  const initial = store.getSnapshot();
  expect(initial.files).toHaveLength(1);
  expect(initial.complete).toBe(false);
  store.accept({ type: "annotations", file: file.file, annotations: [{ region_id: 3, label: "Later summary" }] });
  const updated = store.getSnapshot().files[0];
  if (updated.diff.type !== "text" || file.diff.type !== "text") throw Error("Expected text");
  expect(updated.diff.rhs!.regions[2].visibility.label).toBe("Later summary");
  expect(file.diff.rhs!.regions[2].visibility.label).toBe("");
  expect(updated.diff.rhs!.regions[2].visibility.collapsed).toBe(false);
  expect(updated.diff.stats).toBe(file.diff.stats);
  expect(updated.diff.rhs!.text).toBe(file.diff.rhs!.text);
  store.accept({ type: "annotations", file: file.file, annotations: [], error: { code: "failed", message: "offline" } });
  expect(store.getSnapshot().files[0]).toBe(updated);
  expect(store.getSnapshot().failedFiles.size).toBe(0);
  expect(store.getSnapshot().errors[0]).toContain("summaries unavailable");
});
