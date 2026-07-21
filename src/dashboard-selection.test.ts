import { describe, expect, it } from "vitest";

import { aggregateRangeSummary, averageLatestLatency } from "./dashboard-selection";
import type { HistorySeries, LiveTargetStatus, RangeSummary, Target } from "./types";

const target: Target = {
  id: "target",
  name: "Target",
  host: "example.com",
  enabled: true,
  addressFamily: "auto",
  intervalMs: 1_000,
  timeoutMs: 1_000,
  thresholds: {
    windowSeconds: 60,
    minimumSamples: 10,
    packetLossPercent: 5,
    jitterMs: 30,
    p95LatencyMs: 150,
    unstableForSeconds: 10,
    stableForSeconds: 30,
    outageFailures: 5,
    recoverySuccesses: 3,
  },
  createdAtMs: 0,
  archivedAtMs: null,
};

describe("all-monitor selection", () => {
  it("averages current latency across targets with replies", () => {
    const status = (id: string, latencyMs: number | null): LiveTargetStatus => ({
      target: { ...target, id },
      state: "stable",
      stateSinceMs: 0,
      latestSample: {
        targetId: id,
        timestampMs: 0,
        latencyMs,
        status: latencyMs == null ? "timeout" : "success",
        resolvedAddress: null,
        error: null,
      },
      metrics: {
        sampleCount: 1,
        successCount: latencyMs == null ? 0 : 1,
        packetLossPercent: latencyMs == null ? 100 : 0,
        averageLatencyMs: latencyMs,
        minimumLatencyMs: latencyMs,
        maximumLatencyMs: latencyMs,
        p95LatencyMs: latencyMs,
        jitterMs: null,
      },
      reasons: [],
    });

    expect(averageLatestLatency([status("a", 20), status("b", null), status("c", 40)])).toBe(
      30,
    );
  });

  it("combines monitor-time and shows the worst per-monitor P95", () => {
    const summary = (overrides: Partial<RangeSummary>): RangeSummary => ({
      sampleCount: 10,
      successCount: 9,
      failureCount: 1,
      packetLossPercent: 10,
      averageLatencyMs: 20,
      minimumLatencyMs: 10,
      maximumLatencyMs: 100,
      p95LatencyMs: 80,
      stableMs: 800,
      unstableMs: 100,
      disconnectedMs: 100,
      stablePercent: 80,
      unstablePercent: 10,
      disconnectedPercent: 10,
      ...overrides,
    });
    const series = (id: string, value: RangeSummary): HistorySeries => ({
      target: { ...target, id },
      points: [],
      intervals: [],
      summary: value,
    });

    const aggregate = aggregateRangeSummary([
      series("a", summary({ averageLatencyMs: 20, p95LatencyMs: 80 })),
      series(
        "b",
        summary({
          successCount: 1,
          failureCount: 9,
          averageLatencyMs: 100,
          p95LatencyMs: 300,
          stableMs: 500,
          unstableMs: 300,
          disconnectedMs: 200,
        }),
      ),
    ]);

    expect(aggregate).toMatchObject({
      sampleCount: 20,
      successCount: 10,
      failureCount: 10,
      packetLossPercent: 50,
      averageLatencyMs: 28,
      p95LatencyMs: 300,
      unstablePercent: 20,
      disconnectedPercent: 15,
    });
  });
});
