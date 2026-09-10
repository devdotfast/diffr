import { expect, test } from "bun:test";
import { ReviewSummaryCard } from "../src/components/ReviewSummaryCard";
import { reviewButtonLabel, reviewTimestampLabel } from "../src/lib/reviewCopy";

test("switches the card copy to review-oriented labels", () => {
  expect(ReviewSummaryCard).toBeDefined();
  expect(reviewButtonLabel(3)).toBe("Review 3 files");
  expect(reviewTimestampLabel("2m ago")).toBe("Updated 2m ago");
});
