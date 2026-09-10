import { expect, test, spyOn } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { act } from "react";

import { App } from "./App";
import { DiffStore } from "../diffr/store";
import { createTestDiffFile } from "../diffr/fixture";
test("render real OpenTUI rows, switch layout, collapse and reopen file with mouse", async () => {
  const store = new DiffStore();
  store.accept(createTestDiffFile());
  const testRenderer = await testRender(
    <App store={store} onQuit={() => {}} />,
    { width: 150, height: 20 },
  );
  try {
    await act(async () => {
      await testRenderer.renderOnce();
    });
    await testRenderer.waitForFrame((frame) => frame.includes('send("old")'));
    expect(testRenderer.captureCharFrame()).toContain('send("new")');
    expect(testRenderer.captureCharFrame()).toContain("split [s]");
    const clipboard = spyOn(
      testRenderer.renderer,
      "copyToClipboardOSC52",
    ).mockReturnValue(true);
    await act(async () => {
      await testRenderer.mockMouse.drag(40, 2, 40, 5);
    });
    await act(async () => {
      testRenderer.mockInput.pressKey("y");
    });
    expect(clipboard).toHaveBeenCalledWith('start();\nsend("old");\nfinish();');
    clipboard.mockRestore();
    await act(async () => {
      testRenderer.mockInput.pressKey("s");
    });
    await testRenderer.waitForFrame((frame) => frame.includes("unified [s]"));
    const frame = testRenderer.captureCharFrame();
    expect(frame.indexOf('send("old")')).toBeLessThan(
      frame.indexOf('send("new")'),
    );
    await act(async () => {
      await testRenderer.mockMouse.click(32, 1);
    });
    await testRenderer.waitForFrame(
      (frame) => frame.includes("▸") && !frame.includes('send("old")'),
    );
    await act(async () => {
      await testRenderer.mockMouse.click(32, 1);
    });
    await testRenderer.waitForFrame((frame) => frame.includes('send("old")'));
  } finally {
    await act(async () => {
      testRenderer.renderer.destroy();
    });
  }
});
test("scrolling a large stream keeps terminal renderables bounded", async () => {
  const store = new DiffStore(),
    file = createTestDiffFile();
  const lines = Array.from({ length: 5000 }, (_, i) => `line ${i}`);
  file.diff.lhs_src = file.diff.rhs_src = { Text: lines.join("\n") };
  file.diff.lhs_positions = file.diff.rhs_positions = [];
  file.diff.hunks = [
    { novel_lhs: [], novel_rhs: [], lines: lines.map((_, i) => [i, i]) },
  ];
  file.diff.aligned_rows = lines.map((_, i) => [i, i]);
  store.accept(file);
  const testRenderer = await testRender(
    <App store={store} onQuit={() => {}} />,
    { width: 150, height: 20 },
  );
  try {
    await act(async () => {
      await testRenderer.renderOnce();
    });
    await testRenderer.waitForFrame((frame) => frame.includes("line 0"));
    await act(async () => {
      testRenderer.mockInput.pressKey("END");
    });
    await testRenderer.waitForFrame((frame) => frame.includes("line 4999"));
    function count(node: { getChildren(): any[] }): number {
      return 1 + node.getChildren().reduce((n, c) => n + count(c), 0);
    }
    expect(count(testRenderer.renderer.root)).toBeLessThan(250);
    await act(async () => {
      await testRenderer.mockMouse.scroll(70, 8, "up");
    });
    await testRenderer.waitForFrame((frame) => !frame.includes("line 4999"));
  } finally {
    await act(async () => {
      testRenderer.renderer.destroy();
    });
  }
});
test("hierarchical tree navigation, sticky counts, sidebar toggle and menus", async () => {
  const store = new DiffStore();
  for (const path of ["src/alpha.ts", "src/nested/beta.ts"]) {
    const file = createTestDiffFile();
    file.file.new_path = path;
    const lines = Array.from({length: 60}, (_, i) => `code ${i}`);
    file.diff.lhs_src = file.diff.rhs_src = {Text: lines.join("\n")};
    file.diff.lhs_positions = file.diff.rhs_positions = [];
    file.diff.aligned_rows = lines.map((_, i) => [i, i]);
    file.diff.hunks = [{novel_lhs: [20], novel_rhs: [20,21], lines: file.diff.aligned_rows}];
    store.accept(file);
  }
  const t = await testRender(<App store={store} onQuit={() => {}} />, {width:150, height:20});
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame(f => f.includes("▾ src"));
    expect(t.captureCharFrame()).toContain("▾ nested");
    // Sorted tree: src / nested / beta.ts / alpha.ts.
    await act(async () => { await t.mockMouse.click(8, 3); });
    await t.waitForFrame(f => f.split("\n")[1].includes("src/nested/beta.ts"));
    await act(async () => { t.mockInput.pressKey("\x1b[6~"); });
    await t.waitForFrame(f => f.includes("code 20"));
    expect(t.captureCharFrame().split("\n")[1]).toContain("src/nested/beta.ts");
    expect(t.captureCharFrame().split("\n")[1]).toContain("+2 -1");
    await act(async () => { t.mockInput.pressKey("\\"); });
    await t.waitForFrame(f => !f.includes("▾ src "));
    expect(t.captureCharFrame().split("\n")[1].trimStart()).toStartWith("▾ src/nested/beta.ts");
    await act(async () => { t.mockInput.pressKey("\\"); });
    await t.waitForFrame(f => f.includes("▾ nested"));
    await act(async () => { await t.mockMouse.click(9, 0); });
    await t.waitForFrame(f => f.includes("Context: compact"));
    await act(async () => { t.mockInput.pressKey("ESCAPE"); await new Promise(resolve => setTimeout(resolve, 50)); });
    await t.waitForFrame(f => !f.includes("Context: compact"));
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
test("Hunk navigation chords and draggable sidebar preserve viewport behavior", async () => {
  const store = new DiffStore(), file = createTestDiffFile();
  const lines = Array.from({length: 150}, (_, i) => `row ${i}`);
  file.diff.lhs_src = file.diff.rhs_src = {Text: lines.join("\n")};
  file.diff.lhs_positions = file.diff.rhs_positions = [];
  file.diff.aligned_rows = lines.map((_, i) => [i, i]);
  file.diff.hunks = [{novel_lhs: [], novel_rhs: [], lines: file.diff.aligned_rows}];
  store.accept(file);
  const t = await testRender(<App store={store} onQuit={() => {}} />, {width:150, height:20});
  const firstSource = () => Number(t.captureCharFrame().split("\n")[2].match(/row (\d+)/)?.[1]);
  const press = async (name: string, ctrl = false) => {
    await act(async () => { t.mockInput.pressKey(name, {ctrl}); });
    await t.renderOnce();
  };
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame(f => f.includes("row 0"));
    await press("d", true);
    expect(firstSource()).toBe(9);
    await press("u", true);
    expect(firstSource()).toBe(0);
    await press("f", true);
    expect(firstSource()).toBe(18);
    await press("b", true);
    expect(firstSource()).toBe(0);
    await press("f");
    await press("b");
    expect(firstSource()).toBe(0);
    await press("G");
    expect(t.captureCharFrame()).toContain("row 149");
    await press("g");
    expect(firstSource()).toBe(0);
    expect(t.captureCharFrame().split("\n")[2].indexOf("│")).toBe(27);
    await act(async () => { await t.mockMouse.drag(27, 8, 47, 8); });
    await t.waitForFrame(f => f.split("\n")[2].indexOf("│") === 47);
    await press("\\");
    await press("\\");
    expect(t.captureCharFrame().split("\n")[2].indexOf("│")).toBe(47);
    await act(async () => { await t.mockMouse.drag(47, 8, 2, 8); });
    await t.waitForFrame(f => f.split("\n")[2].indexOf("│") === 15);
    expect(firstSource()).toBe(0);
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
test("initial manifest renders pending tree and remembers a jump until its diff arrives", async () => {
  const store = new DiffStore(), a = createTestDiffFile(), b = createTestDiffFile();
  a.file = {...a.file, old_path:"src/a.ts", new_path:"src/a.ts"};
  b.file = {...b.file, old_path:"src/b.ts", new_path:"src/b.ts"};
  store.accept({type:"start", version:1, before:{kind:"index"}, after:{kind:"working_tree"},
    total:2, files:[a.file,b.file]});
  const t = await testRender(<App store={store} onQuit={() => {}} />, {width:150, height:20});
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame(f => f.includes("◌ a.ts") && f.includes("◌ b.ts"));
    expect(t.captureCharFrame()).not.toContain('send("old")');
    await act(async () => { await t.mockMouse.click(8,3); });
    await t.waitForFrame(f => f.includes("Waiting for b.ts"));
    await act(async () => { store.accept(a); });
    await t.waitForFrame(f => f.includes("◌ b.ts"));
    await act(async () => { store.accept(b); });
    await t.waitForFrame(f => f.split("\n")[1].includes("src/b.ts"));
    expect(t.captureCharFrame()).not.toContain("◌ b.ts");
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
