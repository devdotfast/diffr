import { expect, test } from "bun:test";
import { buildFileTree, flattenFileTree, parentDirectories, lineCounts } from "./fileTree";
import { createTestDiffFile } from "./fixture";
test("tree shares directories, retains file identities, and folds subtrees", () => {
  const files = ["src/ui/App.tsx", "README.md", "src/core.rs", "test/ui/App.tsx"].map(path => {
    const file = createTestDiffFile();
    file.file.new_path = path;
    return file;
  });
  const tree = buildFileTree(files);
  expect(flattenFileTree(tree, new Set()).map(r => [r.node.name, r.depth, r.node.fileIndex])).toEqual([
    ["src", 0, undefined], ["ui", 1, undefined], ["App.tsx", 2, 0], ["core.rs", 1, 2],
    ["test", 0, undefined], ["ui", 1, undefined], ["App.tsx", 2, 3], ["README.md", 0, 1],
  ]);
  expect(flattenFileTree(tree, new Set(["/src"])).some(r => r.node.fileIndex === 0)).toBe(false);
  expect(parentDirectories(files[0])).toEqual(["/src", "/src/ui"]);
  files[0].diff.hunks.push(files[0].diff.hunks[0]);
  expect(lineCounts(files[0])).toEqual({added: 2, removed: 1});
});
