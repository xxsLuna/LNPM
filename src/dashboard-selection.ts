import type { HistorySeries, LiveTargetStatus, RangeSummary } from "./types";

export function averageLatestLatency(targets: readonly LiveTargetStatus[]): number | null {
  const values = targets
    .map((target) => target.latestSample?.latencyMs)
    .filter((value): value is number => value != null);
  if (values.length === 0) return null;
  return values.reduce((total, value) => total + value, 0) / values.length;
}

export function aggregateRangeSummary(
  series: readonly HistorySeries[],
): RangeSummary | undefined {
  if (series.length === 0) return undefined;
  const summaries = series.map((item) => item.summary);
  const sampleCount = sum(summaries.map((summary) => summary.sampleCount));
  const successCount = sum(summaries.map((summary) => summary.successCount));
  const failureCount = sum(summaries.map((summary) => summary.failureCount));
  const stableMs = sum(summaries.map((summary) => summary.stableMs));
  const unstableMs = sum(summaries.map((summary) => summary.unstableMs));
  const disconnectedMs = sum(summaries.map((summary) => summary.disconnectedMs));
  const observedMs = stableMs + unstableMs + disconnectedMs;

  return {
    sampleCount,
    successCount,
    failureCount,
    packetLossPercent: sampleCount === 0 ? 0 : (failureCount / sampleCount) * 100,
    averageLatencyMs: weightedAverage(
      summaries.map((summary) => [summary.averageLatencyMs, summary.successCount]),
    ),
    minimumLatencyMs: minimum(summaries.map((summary) => summary.minimumLatencyMs)),
    maximumLatencyMs: maximum(summaries.map((summary) => summary.maximumLatencyMs)),
    // The exact combined P95 requires raw samples. The highest per-monitor P95
    // is deterministic and surfaces the worst monitored path in the overview.
    p95LatencyMs: maximum(summaries.map((summary) => summary.p95LatencyMs)),
    stableMs,
    unstableMs,
    disconnectedMs,
    stablePercent: percentage(stableMs, observedMs),
    unstablePercent: percentage(unstableMs, observedMs),
    disconnectedPercent: percentage(disconnectedMs, observedMs),
  };
}

function sum(values: readonly number[]): number {
  return values.reduce((total, value) => total + value, 0);
}

function weightedAverage(values: readonly [number | null, number][]): number | null {
  const usable = values.filter(
    (entry): entry is [number, number] => entry[0] != null && entry[1] > 0,
  );
  const weight = sum(usable.map(([, count]) => count));
  if (weight === 0) return null;
  return usable.reduce((total, [value, count]) => total + value * count, 0) / weight;
}

function minimum(values: readonly (number | null)[]): number | null {
  const usable = values.filter((value): value is number => value != null);
  return usable.length === 0 ? null : Math.min(...usable);
}

function maximum(values: readonly (number | null)[]): number | null {
  const usable = values.filter((value): value is number => value != null);
  return usable.length === 0 ? null : Math.max(...usable);
}

function percentage(value: number, total: number): number {
  return total === 0 ? 0 : (value / total) * 100;
}
