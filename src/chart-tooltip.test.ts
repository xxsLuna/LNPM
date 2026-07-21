import { describe, expect, it } from "vitest";

import { calculateTooltipPosition } from "./chart-tooltip";

describe("calculateTooltipPosition", () => {
  it("places the tooltip beside the cursor", () => {
    expect(
      calculateTooltipPosition({
        anchorX: 300,
        anchorY: 180,
        tooltipWidth: 140,
        tooltipHeight: 100,
        containerWidth: 800,
        containerHeight: 400,
      }),
    ).toEqual({ left: 312, top: 130 });
  });

  it("flips to the left near the right edge", () => {
    expect(
      calculateTooltipPosition({
        anchorX: 760,
        anchorY: 180,
        tooltipWidth: 140,
        tooltipHeight: 100,
        containerWidth: 800,
        containerHeight: 400,
      }),
    ).toEqual({ left: 608, top: 130 });
  });

  it("keeps the tooltip inside the chart near every edge", () => {
    expect(
      calculateTooltipPosition({
        anchorX: 4,
        anchorY: 4,
        tooltipWidth: 140,
        tooltipHeight: 100,
        containerWidth: 160,
        containerHeight: 120,
      }),
    ).toEqual({ left: 12, top: 8 });
  });
});
