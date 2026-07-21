import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

import { calculateTooltipPosition } from "./chart-tooltip";
import { formatDateTime, formatLatency, stateLabel } from "./i18n";
import type { HistoryResponse, QualityIntervalRecord } from "./types";

const palette = ["#5eead4", "#60a5fa", "#c084fc", "#f472b6", "#facc15"];

interface ChartOptions {
  compact?: boolean;
  selectedTargetId?: string | null;
  onRangeChanged?: (fromMs: number, toMs: number) => void;
}

export class LatencyChart {
  private plot: uPlot | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private tooltip: HTMLDivElement;
  private history: HistoryResponse | null = null;
  private selectedTargetId: string | null;

  constructor(
    private readonly container: HTMLElement,
    private readonly options: ChartOptions = {},
  ) {
    this.selectedTargetId = options.selectedTargetId ?? null;
    this.tooltip = document.createElement("div");
    this.tooltip.className = "chart-tooltip";
    this.container.append(this.tooltip);
  }

  render(history: HistoryResponse, selectedTargetId = this.selectedTargetId): void {
    this.history = history;
    this.selectedTargetId = selectedTargetId ?? null;
    this.plot?.destroy();
    this.resizeObserver?.disconnect();
    this.container.querySelector(".uplot")?.remove();

    const { data, labels } = alignSeries(history);
    const width = Math.max(280, this.container.clientWidth);
    const height = Math.max(this.options.compact ? 80 : 260, this.container.clientHeight);
    const intervals = this.intervalsForDisplay(history);

    const series: uPlot.Series[] = [
      { label: "Time" },
      ...history.series.map((item, index) => ({
        label: item.target.name,
        stroke: palette[index % palette.length],
        width: item.target.id === this.selectedTargetId ? 2.6 : 1.7,
        alpha:
          this.selectedTargetId && item.target.id !== this.selectedTargetId && !this.options.compact
            ? 0.35
            : 1,
        spanGaps: false,
        points: { show: false },
        value: (_self: uPlot, rawValue: number | null) => formatLatency(rawValue),
      })),
    ];

    const plotOptions: uPlot.Options = {
      width,
      height,
      padding: this.options.compact ? [8, 6, 4, 0] : [12, 18, 4, 8],
      scales: {
        x: { time: true },
        y: {
          auto: true,
          range: (_u, _min, max) => [0, Math.max(50, (max || 50) * 1.15)],
        },
      },
      series,
      axes: this.options.compact
        ? [{ show: false }, { show: false }]
        : [
            {
              stroke: "#7f8da3",
              grid: { stroke: "rgba(148, 163, 184, 0.10)", width: 1 },
            },
            {
              stroke: "#7f8da3",
              label: "ms",
              grid: { stroke: "rgba(148, 163, 184, 0.10)", width: 1 },
            },
          ],
      cursor: {
        drag: { x: false, y: false },
        focus: { prox: 24 },
        points: { size: 7, width: 2 },
      },
      legend: { show: false },
      hooks: {
        drawClear: [(u) => drawIntervals(u, intervals, history.toMs)],
        setCursor: [(u) => this.updateTooltip(u, labels, intervals)],
        ready: [(u) => this.attachInteractions(u)],
      },
    };

    this.plot = new uPlot(plotOptions, data, this.container);
    this.resizeObserver = new ResizeObserver(() => {
      if (!this.plot) return;
      const nextWidth = Math.max(280, this.container.clientWidth);
      const nextHeight = Math.max(
        this.options.compact ? 80 : 260,
        this.container.clientHeight,
      );
      if (this.plot.width !== nextWidth || this.plot.height !== nextHeight) {
        this.plot.setSize({ width: nextWidth, height: nextHeight });
      }
    });
    this.resizeObserver.observe(this.container);
  }

  destroy(): void {
    this.resizeObserver?.disconnect();
    this.plot?.destroy();
    this.plot = null;
    this.tooltip.remove();
  }

  private intervalsForDisplay(history: HistoryResponse): QualityIntervalRecord[] {
    if (this.options.compact) {
      return history.series.flatMap((series) => series.intervals);
    }
    if (this.selectedTargetId === null) {
      return history.series.flatMap((series) => series.intervals);
    }
    const selected =
      history.series.find((series) => series.target.id === this.selectedTargetId) ??
      history.series[0];
    return selected?.intervals ?? [];
  }

  private updateTooltip(
    plot: uPlot,
    labels: string[],
    intervals: QualityIntervalRecord[],
  ): void {
    const index = plot.cursor.idx;
    if (index == null || plot.cursor.left == null || !this.history) {
      this.tooltip.classList.remove("visible");
      return;
    }
    const timestampSeconds = plot.data[0][index];
    if (timestampSeconds == null) return;
    const timestampMs = timestampSeconds * 1_000;
    const values = labels
      .map((label, seriesIndex) => {
        const value = plot.data[seriesIndex + 1]?.[index];
        return value == null
          ? ""
          : `<div><span class="tooltip-swatch" style="--swatch:${palette[seriesIndex % palette.length]}"></span>${escapeHtml(label)} <strong>${formatLatency(value)}</strong></div>`;
      })
      .filter(Boolean)
      .join("");
    const interval = intervals.find(
      (item) => timestampMs >= item.startMs && timestampMs <= (item.endMs ?? this.history!.toMs),
    );
    const intervalText = interval
      ? `<div class="tooltip-state state-${interval.state}">${stateLabel(interval.state)}</div>`
      : "";
    this.tooltip.innerHTML = `<time>${formatDateTime(timestampMs)}</time>${values}${intervalText}`;
    const containerRect = this.container.getBoundingClientRect();
    const overlayRect = plot.over.getBoundingClientRect();
    const anchorX = overlayRect.left - containerRect.left + plot.cursor.left;
    const anchorY =
      overlayRect.top - containerRect.top + (plot.cursor.top ?? overlayRect.height / 2);
    const position = calculateTooltipPosition({
      anchorX,
      anchorY,
      tooltipWidth: this.tooltip.offsetWidth,
      tooltipHeight: this.tooltip.offsetHeight,
      containerWidth: this.container.clientWidth,
      containerHeight: this.container.clientHeight,
    });
    this.tooltip.style.left = `${position.left}px`;
    this.tooltip.style.top = `${position.top}px`;
    this.tooltip.classList.add("visible");
  }

  private attachInteractions(plot: uPlot): void {
    if (this.options.compact) return;
    const overlay = plot.over;
    let dragging = false;
    let startX = 0;
    let startMin = 0;
    let startMax = 0;

    overlay.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || plot.scales.x.min == null || plot.scales.x.max == null) return;
      dragging = true;
      startX = event.clientX;
      startMin = plot.scales.x.min;
      startMax = plot.scales.x.max;
      overlay.setPointerCapture(event.pointerId);
      overlay.classList.add("is-panning");
    });
    overlay.addEventListener("pointermove", (event) => {
      if (!dragging) return;
      const deltaSeconds =
        ((event.clientX - startX) / Math.max(1, plot.bbox.width)) * (startMax - startMin);
      plot.setScale("x", { min: startMin - deltaSeconds, max: startMax - deltaSeconds });
    });
    const finishPan = (event: PointerEvent) => {
      if (!dragging) return;
      dragging = false;
      overlay.releasePointerCapture(event.pointerId);
      overlay.classList.remove("is-panning");
      this.reportRange(plot);
    };
    overlay.addEventListener("pointerup", finishPan);
    overlay.addEventListener("pointercancel", finishPan);
    overlay.addEventListener(
      "wheel",
      (event) => {
        if (plot.scales.x.min == null || plot.scales.x.max == null) return;
        event.preventDefault();
        const range = plot.scales.x.max - plot.scales.x.min;
        const factor = event.deltaY > 0 ? 1.25 : 0.8;
        const cursorRatio = Math.max(0, Math.min(1, event.offsetX / Math.max(1, plot.bbox.width)));
        const anchor = plot.scales.x.min + range * cursorRatio;
        const nextRange = Math.max(60, Math.min(365 * 86_400, range * factor));
        plot.setScale("x", {
          min: anchor - nextRange * cursorRatio,
          max: anchor + nextRange * (1 - cursorRatio),
        });
        window.clearTimeout((overlay as HTMLElement & { zoomTimer?: number }).zoomTimer);
        (overlay as HTMLElement & { zoomTimer?: number }).zoomTimer = window.setTimeout(
          () => this.reportRange(plot),
          180,
        );
      },
      { passive: false },
    );
  }

  private reportRange(plot: uPlot): void {
    const min = plot.scales.x.min;
    const max = plot.scales.x.max;
    if (min != null && max != null) this.options.onRangeChanged?.(min * 1_000, max * 1_000);
  }
}

function alignSeries(history: HistoryResponse): {
  data: uPlot.AlignedData;
  labels: string[];
} {
  const timestamps = Array.from(
    new Set(history.series.flatMap((series) => series.points.map((point) => point.timestampMs))),
  ).sort((a, b) => a - b);
  if (timestamps.length === 0) timestamps.push(history.fromMs, history.toMs);
  const data: uPlot.AlignedData = [timestamps.map((timestamp) => timestamp / 1_000)];
  for (const series of history.series) {
    const values = new Map(
      series.points.map((point) => [point.timestampMs, point.averageLatencyMs] as const),
    );
    data.push(timestamps.map((timestamp) => values.get(timestamp) ?? null));
  }
  return { data, labels: history.series.map((series) => series.target.name) };
}

function drawIntervals(
  plot: uPlot,
  intervals: QualityIntervalRecord[],
  fallbackEndMs: number,
): void {
  const drawable = intervals
    .filter((interval) => ["unstable", "disconnected", "paused", "unobserved"].includes(interval.state))
    .sort((a, b) => intervalPriority(a) - intervalPriority(b));
  for (const interval of drawable) {
    const start = plot.valToPos(interval.startMs / 1_000, "x", true);
    const end = plot.valToPos((interval.endMs ?? fallbackEndMs) / 1_000, "x", true);
    const left = Math.max(plot.bbox.left, Math.min(start, end));
    const right = Math.min(plot.bbox.left + plot.bbox.width, Math.max(start, end));
    if (right <= left) continue;
    plot.ctx.fillStyle = intervalColor(interval);
    plot.ctx.fillRect(left, plot.bbox.top, right - left, plot.bbox.height);
  }
}

function intervalPriority(interval: QualityIntervalRecord): number {
  return interval.state === "disconnected" ? 2 : interval.state === "unstable" ? 1 : 0;
}

function intervalColor(interval: QualityIntervalRecord): string {
  if (interval.state === "disconnected") return "rgba(239, 68, 68, 0.20)";
  if (interval.state === "unstable") return "rgba(245, 158, 11, 0.16)";
  return "rgba(100, 116, 139, 0.12)";
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>'"]/g, (character) => {
    const entities: Record<string, string> = {
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      "'": "&#39;",
      '"': "&quot;",
    };
    return entities[character];
  });
}
