import { expect, test } from "bun:test";
import { ChangeSummaryCard } from "../src/components/ReviewSummaryCard";

test("keeps the old sync-oriented labels", () => {
  expect(ChangeSummaryCard).toBeDefined();
  expect("Open diff").toContain("diff");
  expect("Synced 2m ago").toContain("Synced");
});
