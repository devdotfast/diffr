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
