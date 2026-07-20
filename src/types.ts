export type AddressFamily = "auto" | "ipv4" | "ipv6";
export type ProbeStatus =
  | "success"
  | "timeout"
  | "unreachable"
  | "dnsError"
  | "permissionDenied"
  | "error";
export type QualityState =
  | "warmingUp"
  | "stable"
  | "unstable"
  | "disconnected"
  | "paused"
  | "unobserved"
  | "error";
export type QualityReason =
  | "packetLoss"
  | "jitter"
  | "highLatency"
  | "consecutiveFailures"
  | "configuration";

export interface QualityThresholds {
  windowSeconds: number;
  minimumSamples: number;
  packetLossPercent: number;
  jitterMs: number;
  p95LatencyMs: number;
  unstableForSeconds: number;
  stableForSeconds: number;
  outageFailures: number;
  recoverySuccesses: number;
}

export interface Target {
  id: string;
  name: string;
  host: string;
  enabled: boolean;
  addressFamily: AddressFamily;
  intervalMs: number;
  timeoutMs: number;
  thresholds: QualityThresholds;
  createdAtMs: number;
  archivedAtMs: number | null;
}

export interface PingSample {
  targetId: string;
  timestampMs: number;
  latencyMs: number | null;
  status: ProbeStatus;
  resolvedAddress: string | null;
  error: string | null;
}

export interface QualityMetrics {
  sampleCount: number;
  successCount: number;
  packetLossPercent: number;
  averageLatencyMs: number | null;
  minimumLatencyMs: number | null;
  maximumLatencyMs: number | null;
  p95LatencyMs: number | null;
  jitterMs: number | null;
}

export interface LiveTargetStatus {
  target: Target;
  state: QualityState;
  stateSinceMs: number;
  latestSample: PingSample | null;
  metrics: QualityMetrics;
  reasons: QualityReason[];
}

export interface DashboardSnapshot {
  nowMs: number;
  paused: boolean;
  targets: LiveTargetStatus[];
}

export interface StateTransition {
  from: QualityState;
  to: QualityState;
  effectiveAtMs: number;
  reasons: QualityReason[];
}

export interface QualityTransitionEvent {
  target: Target;
  transition: StateTransition;
  metrics: QualityMetrics;
}

export interface HistoryPoint {
  timestampMs: number;
  averageLatencyMs: number | null;
  minimumLatencyMs: number | null;
  maximumLatencyMs: number | null;
  sampleCount: number;
  failureCount: number;
}

export interface QualityIntervalRecord {
  startMs: number;
  endMs: number | null;
  state: QualityState;
  reasons: QualityReason[];
}

export interface RangeSummary {
  sampleCount: number;
  successCount: number;
  failureCount: number;
  packetLossPercent: number;
  averageLatencyMs: number | null;
  minimumLatencyMs: number | null;
  maximumLatencyMs: number | null;
  p95LatencyMs: number | null;
  stableMs: number;
  unstableMs: number;
  disconnectedMs: number;
  stablePercent: number;
  unstablePercent: number;
  disconnectedPercent: number;
}

export interface HistorySeries {
  target: Target;
  points: HistoryPoint[];
  intervals: QualityIntervalRecord[];
  summary: RangeSummary;
}

export interface HistoryResponse {
  fromMs: number;
  toMs: number;
  bucketMs: number;
  series: HistorySeries[];
}

export interface AppSettings {
  retentionDays: number | null;
  notificationsEnabled: boolean;
  startAtLogin: boolean;
  language: "auto" | "ko" | "en";
  firstRun: boolean;
}

export interface StorageInfo {
  dataDirectory: string;
  databasePath: string;
  databaseSizeBytes: number;
}
