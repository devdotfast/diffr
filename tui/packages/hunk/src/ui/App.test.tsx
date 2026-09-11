import { expect, test, spyOn } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { act, Profiler } from "react";
import { TextRenderable, type BaseRenderable } from "@opentui/core";

import { App } from "./App";
import { DiffStore } from "../diffr/store";
import { createTestDiffFile, leaf, line, withIdenticalLines } from "../diffr/fixture";
import type { DiffFile } from "../diffr/wire";
import { createFoldedDiffFile } from "../diffr/regions.test";
import { loadBundledTheme } from "../diffr/theme";
const themes = { initial: loadBundledTheme("default-dark"), dark: loadBundledTheme("default-dark"), light: loadBundledTheme("default-light") };
const at = (file: DiffFile, path: string) => {
  file.file = { lhs: { path, oid: "1", mode: "100644" }, rhs: { path, oid: "2", mode: "100644" } };
  return file;
};
test("render real OpenTUI rows, switch layout, collapse and reopen file with mouse", async () => {
  const store = new DiffStore();
  store.accept(createTestDiffFile());
  const testRenderer = await testRender(
    <App store={store} onQuit={() => {}} themes={themes} />,
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
      await testRenderer.mockMouse.drag(40, 3, 40, 6);
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
      await testRenderer.mockMouse.click(32, 2);
    });
    await testRenderer.waitForFrame(
      (frame) => frame.includes("▸") && !frame.includes('send("old")'),
    );
    await act(async () => {
      await testRenderer.mockMouse.click(32, 2);
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
  withIdenticalLines(file, 5000);
  store.accept(file);
  const testRenderer = await testRender(
    <App store={store} onQuit={() => {}} themes={themes} />,
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
      await testRenderer.mockMouse.scroll(70, 9, "up");
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
    const file = at(createTestDiffFile(), path);
    const lines = Array.from({length: 60}, (_, i) => `code ${i}`);
    if (file.diff.type !== "text") throw new Error();
    file.diff.lhs = { text: lines.join("\n"), syntax: [], regions: [leaf(1, 0, 20), leaf(2, 20, 21, [line(20, 0, 7)]), leaf(3, 21, 60)] };
    file.diff.rhs = { text: lines.join("\n"), syntax: [], regions: [leaf(1, 0, 20), leaf(2, 20, 21, [line(20, 0, 7)]), leaf(4, 21, 22, [line(21, 0, 7)]), leaf(3, 22, 60)] };
    store.accept(file);
  }
  const t = await testRender(<App store={store} onQuit={() => {}} themes={themes} />, {width:150, height:20});
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame(f => f.includes("▾ src"));
    expect(t.captureCharFrame()).toContain("▾ nested");
    // Sorted tree: src / nested / beta.ts / alpha.ts.
    await act(async () => { await t.mockMouse.click(8, 4); });
    await t.waitForFrame(f => f.split("\n")[2].includes("src/nested/beta.ts"));
    await act(async () => { t.mockInput.pressKey("\x1b[6~"); });
    await t.waitForFrame(f => f.includes("code 20"));
    expect(t.captureCharFrame().split("\n")[2]).toContain("src/nested/beta.ts");
    expect(t.captureCharFrame().split("\n")[2]).toContain("+2 −1");
    await act(async () => { t.mockInput.pressKey("\\"); });
    await t.waitForFrame(f => !f.includes("▾ src "));
    expect(t.captureCharFrame().split("\n")[2].trimStart()).toStartWith("▾ src/nested/beta.ts");
    await act(async () => { t.mockInput.pressKey("\\"); });
    await t.waitForFrame(f => f.includes("▾ nested"));
    await act(async () => { await t.mockMouse.click(9, 0); });
    await t.waitForFrame(f => f.includes("Toggle context gaps"));
    await act(async () => { t.mockInput.pressKey("ESCAPE"); await new Promise(resolve => setTimeout(resolve, 50)); });
    await t.waitForFrame(f => !f.includes("Toggle context gaps"));
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
test("Hunk navigation chords and draggable sidebar preserve viewport behavior", async () => {
  const store = new DiffStore(), file = createTestDiffFile();
  const lines = Array.from({length: 150}, (_, i) => `row ${i}`);
  if (file.diff.type !== "text") throw new Error();
  file.diff.lhs = file.diff.rhs = { text: lines.join("\n"), syntax: [], regions: [leaf(1, 0, 150)] };
  store.accept(file);
  const t = await testRender(<App store={store} onQuit={() => {}} themes={themes} />, {width:150, height:20});
  const firstSource = () => Number(t.captureCharFrame().split("\n")[3].match(/row (\d+)/)?.[1]);
  const press = async (name: string, ctrl = false) => {
    await act(async () => { t.mockInput.pressKey(name, {ctrl}); });
    await t.renderOnce();
  };
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame(f => f.includes("row 0"));
    await press("d", true);
    expect(firstSource()).toBe(8);
    await press("u", true);
    expect(firstSource()).toBe(0);
    await press("f", true);
    expect(firstSource()).toBe(17);
    await press("b", true);
    expect(firstSource()).toBe(0);
    await press("f");
    await press("b");
    expect(firstSource()).toBe(0);
    await press("G");
    expect(t.captureCharFrame()).toContain("row 149");
    await press("g");
    expect(firstSource()).toBe(0);
    expect(t.captureCharFrame().split("\n")[3].indexOf("│")).toBe(27);
    await act(async () => { await t.mockMouse.drag(27, 8, 47, 8); });
    await t.waitForFrame(f => f.split("\n")[3].indexOf("│") === 47);
    await press("\\");
    await press("\\");
    expect(t.captureCharFrame().split("\n")[3].indexOf("│")).toBe(47);
    await act(async () => { await t.mockMouse.drag(47, 8, 2, 8); });
    await t.waitForFrame(f => f.split("\n")[3].indexOf("│") === 15);
    expect(firstSource()).toBe(0);
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
test("initial manifest renders pending tree and remembers a jump until its diff arrives", async () => {
  const store = new DiffStore(), a = createTestDiffFile(), b = createTestDiffFile();
  at(a, "src/a.ts");
  at(b, "src/b.ts");
  const entry = (file: DiffFile) => ({ file: file.file, status: "modified" as const, visibility: { collapsed: false, label: "" } });
  store.accept({type:"start", version:2, lhs:{type:"index"}, rhs:{type:"working_tree"}, files:[entry(a), entry(b)]});
  const t = await testRender(<App store={store} onQuit={() => {}} themes={themes} />, {width:150, height:20});
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame(f => f.includes("◌ a.ts") && f.includes("◌ b.ts"));
    expect(t.captureCharFrame()).not.toContain('send("old")');
    await act(async () => { await t.mockMouse.click(8,4); });
    await t.waitForFrame(f => f.includes("Waiting for b.ts"));
    await act(async () => { store.accept(a); });
    await t.waitForFrame(f => f.includes("◌ b.ts"));
    await act(async () => { store.accept(b); });
    await t.waitForFrame(f => f.split("\n")[2].includes("src/b.ts"));
    expect(t.captureCharFrame()).not.toContain("◌ b.ts");
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
test("streaming diffs follow tree order without moving the visible source row", async () => {
  const store = new DiffStore();
  const files = ["z/last.ts", "a/first.ts", "m/middle.ts"].map(path => {
    const file = at(createTestDiffFile(), path);
    if (file.diff.type !== "text") throw new Error();
    const lines = Array.from({length:50}, (_, i) => `code ${i}`);
    file.diff.lhs = file.diff.rhs = { text: lines.join("\n"), syntax: [], regions: [leaf(1, 0, 50)] };
    return file;
  });
  const entry = (file: DiffFile) => ({ file: file.file, status: "modified" as const, visibility: { collapsed: false, label: "" } });
  store.accept({type:"start", version:2, lhs:{type:"index"}, rhs:{type:"working_tree"}, files:files.map(entry)});
  const t = await testRender(<App store={store} onQuit={() => {}} themes={themes} />, {width:150, height:20});
  const sidebarLines = () => t.captureCharFrame().split("\n").slice(2,9).map(line => line.slice(0,27).trim());
  try {
    await act(async () => { await t.renderOnce(); store.accept(files[0]); store.accept(files[2]); });
    await t.waitForFrame(f => f.includes("m/middle.ts"));
    expect(sidebarLines().slice(0,6)).toEqual(["▾ a", "◌ first.ts", "▾ m", "middle.ts", "▾ z", "last.ts"]);
    await act(async () => { t.mockInput.pressKey("g"); });
    await t.waitForFrame(f => f.split("\n")[2].includes("m/middle.ts"));
    await act(async () => { t.mockInput.pressKey("d", {ctrl:true}); });
    await t.renderOnce();
    const before = t.captureCharFrame().split("\n")[3].slice(28);
    await act(async () => { store.accept(files[1]); });
    await t.waitForFrame(f => !f.includes("◌ first.ts") && f.split("\n")[2].includes("m/middle.ts"));
    expect(t.captureCharFrame().split("\n")[3].slice(28)).toBe(before);
    expect(sidebarLines().slice(0,6)).toEqual(["▾ a", "first.ts", "▾ m", "middle.ts", "▾ z", "last.ts"]);
    await act(async () => { store.accept({type:"complete", succeeded:3, failed:0}); });
    await t.renderOnce();
    expect(t.captureCharFrame().split("\n")[3].slice(28)).toBe(before);
    await act(async () => { t.mockInput.pressKey("g"); });
    await t.waitForFrame(f => f.split("\n")[2].includes("a/first.ts"));
    await act(async () => { await t.mockMouse.click(8,5); });
    await t.waitForFrame(f => f.split("\n")[2].includes("m/middle.ts"));
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});

for (const wrap of [false, true]) for (const unified of [false, true])
test(`stream arrivals preserve code in every commit (wrap=${wrap}, unified=${unified})`, async () => {
  const store = new DiffStore();
  const files = ["m/current.ts", "a/earlier.ts", "z/later.ts"].map(path => {
    const file = at(createTestDiffFile(), path);
    if (file.diff.type !== "text") throw new Error();
    const lines = Array.from({length:50}, (_, i) => `source ${path} ${i} ${"word ".repeat(20)}`);
    file.diff.lhs = file.diff.rhs = { text: lines.join("\n"), syntax: [], regions: [leaf(1, 0, 50)] };
    return file;
  });
  const entry = (file: DiffFile) => ({ file: file.file, status: "modified" as const, visibility: { collapsed: false, label: "" } });
  store.accept({type:"start", version:2, lhs:{type:"index"}, rhs:{type:"working_tree"}, files:files.map(entry)});
  store.accept(files[0]);
  let capture: (() => void) | undefined;
  const commits: string[][] = [];
  const t = await testRender(<Profiler id="viewport" onRender={() => capture?.()}>
    <App store={store} onQuit={() => {}} themes={themes} />
  </Profiler>, {width:150, height:20});
  const sourceCells = (node: BaseRenderable): string[] => {
    if (node instanceof TextRenderable) {
      const text = node.content.chunks.map(chunk => chunk.text).join("");
      return text.includes("source ") || text.includes("word ") ? [text] : [];
    }
    return node.getChildren().flatMap(sourceCells);
  };
  try {
    await act(async () => { await t.renderOnce(); });
    if (unified) await act(async () => { t.mockInput.pressKey("s"); });
    if (wrap) await act(async () => { t.mockInput.pressKey("w"); });
    await act(async () => { t.mockInput.pressKey("d", {ctrl:true}); });
    await t.renderOnce();
    const before = sourceCells(t.renderer.root);
    expect(before.length).toBeGreaterThan(0);
    capture = () => { commits.push(sourceCells(t.renderer.root)); };
    for (const file of files.slice(1)) {
      commits.length = 0;
      await act(async () => { store.accept(file); });
      expect(commits.length).toBeGreaterThan(0);
      for (const cells of commits) expect(cells).toEqual(before);
    }
  } finally {
    capture = undefined;
    await act(async () => { t.renderer.destroy(); });
  }
});
test("folds collapse from the gutter chevron and expand from the placeholder", async () => {
  const store = new DiffStore();
  const file = createFoldedDiffFile();
  // Pad the file past the viewport so the z key can act on a scrolled-to row.
  if (file.diff.type !== "text") throw new Error();
  const tail = Array.from({ length: 20 }, (_, i) => `tail ${i}`);
  for (const source of [file.diff.lhs!, file.diff.rhs!]) {
    source.text += tail.join("\n") + "\n";
    source.regions.push(leaf(99, 8, 28));
  }
  store.accept(file);
  const t = await testRender(<App store={store} onQuit={() => {}} themes={themes} />, { width: 150, height: 20 });
  try {
    await act(async () => { await t.renderOnce(); });
    await t.waitForFrame((f) => f.includes("inner(|| {"));
    const lines = () => t.captureCharFrame().split("\n");
    const rowOf = (needle: string) => lines().findIndex((l) => l.includes(needle));
    const inner = rowOf("inner(|| {");
    // Every foldable row shows its chevron without hovering.
    const chevronX = lines()[inner].indexOf("▾");
    expect(chevronX).toBeGreaterThan(0);
    await act(async () => { await t.mockMouse.click(chevronX, inner); });
    await t.waitForFrame((f) => !f.includes("a();") && f.includes("⋯ Body"));
    expect(t.captureCharFrame()).toContain("});");
    expect(lines()[inner]).toContain("▸");
    const folded = lines()[inner];
    await act(async () => { await t.mockMouse.click(folded.lastIndexOf("⋯") + 1, inner); });
    await t.waitForFrame((f) => f.includes("a();"));
    // Vim chords act on the top row once "fn outer() {" is scrolled to the top.
    const chord = async (...keys: string[]) => {
      for (const key of keys) await act(async () => { t.mockInput.pressKey(key); });
    };
    await chord("j");
    await t.waitForFrame((f) => !f.includes("fn outer() {"));
    await chord("z", "c");
    await t.waitForFrame((f) => !f.includes("inner(|| {") && !f.includes("a();"));
    await chord("z", "c");
    await chord("z", "a");
    await t.waitForFrame((f) => f.includes("inner(|| {"));
    // zC closes recursively, then zo reopens only the outer fold.
    await chord("z", "C");
    await t.waitForFrame((f) => !f.includes("inner(|| {"));
    await chord("z", "o");
    await t.waitForFrame((f) => f.includes("inner(|| {") && !f.includes("a();"));
    await chord("z", "O");
    await t.waitForFrame((f) => f.includes("a();"));
    // zj scrolls to the next fold header; an unknown z command is ignored.
    await chord("z", "j");
    await t.waitForFrame((f) => !f.includes("inner(|| {") && f.includes("a();"));
    await chord("z", "x");
    await chord("k", "k");
    await t.waitForFrame((f) => f.includes("fn outer() {"));
    // Alt-click on the outer chevron folds nested regions too, so reopening keeps them folded.
    const outer = rowOf("fn outer() {");
    await act(async () => { await t.mockMouse.click(chevronX, outer, 0, { modifiers: { alt: true } }); });
    await t.waitForFrame((f) => !f.includes("inner(|| {"));
    await act(async () => { await t.mockMouse.click(chevronX, outer); });
    await t.waitForFrame((f) => f.includes("inner(|| {") && !f.includes("a();"));
    await chord("z", "M");
    await t.waitForFrame((f) => !f.includes("inner(|| {"));
    await chord("z", "R");
    await t.waitForFrame((f) => f.includes("a();"));
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});

test("the summary strip shows visible totals that follow fold state, and i opens the breakdown", async () => {
  const store = new DiffStore();
  store.accept({type:"start", version:2, lhs:{type:"revision", rev:"main"}, rhs:{type:"working_tree"},
    files:[{ file: createFoldedDiffFile().file, status: "modified", visibility: { collapsed: false, label: "" } }]});
  store.accept(createFoldedDiffFile());
  const t = await testRender(<App store={store} onQuit={() => {}} themes={themes} />, { width: 150, height: 24 });
  const render = async () => { await act(async () => { await t.renderOnce(); }); };
  const lines = () => t.captureCharFrame().split("\n");
  try {
    await render();
    await render();
    expect(t.captureCharFrame()).toContain("inner(|| {");
    // One changed rhs line inside the closure body; still loading, so the total is partial.
    expect(lines()[1]).toContain("main…working tree");
    expect(lines()[1]).toContain("1 files");
    expect(lines()[1]).toContain("+1 −0…");
    expect(lines()[1]).toContain("■■■■■");
    expect(lines()[2]).toContain("+1 −0");
    await act(async () => { store.accept({ type: "complete", succeeded: 1, failed: 0 }); });
    await render();
    expect(lines()[1]).not.toContain("−0…");
    // Collapse the closure body: the change is hidden, so the totals and header drop to zero.
    const inner = lines().findIndex((l) => l.includes("inner(|| {"));
    const chevronX = lines()[inner].indexOf("▾");
    await act(async () => { await t.mockMouse.click(chevronX, inner); });
    await render();
    expect(lines()[1]).toContain("+0 −0");
    expect(lines()[1]).toContain("□□□□□");
    expect(lines()[2]).toContain("+0 −0");
    await act(async () => { t.mockInput.pressKey("i"); });
    await render();
    const frame = t.captureCharFrame();
    expect(frame).toContain("All files");
    expect(frame).toContain("visible     +0 −0");
    expect(frame).toContain("structural  +1 −0");
    expect(frame).toContain("textual     +1 −0");
    await act(async () => { t.mockInput.pressKey("ESCAPE"); await new Promise((resolve) => setTimeout(resolve, 100)); });
    await render();
    expect(t.captureCharFrame()).not.toContain("visible     +0");
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
