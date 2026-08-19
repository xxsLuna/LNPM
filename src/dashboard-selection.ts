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
  // Summing monitor-time would let a ten-minute range report thirty minutes of trouble, and
  // averaging it would hide a single monitor's outage behind its healthy peers. Each time metric
  // therefore reports the monitor that was worst for that metric — the same "worst of the fleet"
  // convention the P95 card uses — so every number stays inside the selected range and its label is
  // true of the monitor it came from.
  const observedMs = (summary: RangeSummary): number =>
    summary.stableMs + summary.unstableMs + summary.disconnectedMs;
  // Ranked by the share the card actually shows, with the absolute time as the tie-break so a
  // monitor that was only observed for a moment cannot outrank one that was down for an hour.
  const worstBy = (metric: (summary: RangeSummary) => number): RangeSummary =>
    summaries.reduce((candidate, summary) => {
      const share = observedMs(summary) === 0 ? 0 : metric(summary) / observedMs(summary);
      const leader = observedMs(candidate) === 0 ? 0 : metric(candidate) / observedMs(candidate);
      if (share !== leader) return share > leader ? summary : candidate;
      return metric(summary) > metric(candidate) ? summary : candidate;
    });
  const worstUnstable = worstBy((summary) => summary.unstableMs);
  const worstDisconnected = worstBy((summary) => summary.disconnectedMs);
  // "Worst" for stability means the monitor that spent the most time in trouble, not the one with
  // the most stable time — that would be the best of the fleet. The three percentages therefore have
  // three different denominators, one per reported monitor.
  const worstStable = worstBy((summary) => summary.unstableMs + summary.disconnectedMs);

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
    stableMs: worstStable.stableMs,
    unstableMs: worstUnstable.unstableMs,
    disconnectedMs: worstDisconnected.disconnectedMs,
    stablePercent: percentage(worstStable.stableMs, observedMs(worstStable)),
    unstablePercent: percentage(worstUnstable.unstableMs, observedMs(worstUnstable)),
    disconnectedPercent: percentage(
      worstDisconnected.disconnectedMs,
      observedMs(worstDisconnected),
    ),
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
