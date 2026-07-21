import type { UpdateErrorEvent, UpdateInfo, UpdatePhase, UpdateProgressEvent } from "./types";

export interface UpdateUiState {
  info: UpdateInfo | null;
  phase: UpdatePhase | "idle" | "failed";
  percent: number | null;
  error: UpdateErrorEvent | null;
}

export type UpdateUiEvent =
  | { type: "available"; payload: UpdateInfo }
  | { type: "progress"; payload: UpdateProgressEvent }
  | { type: "failed"; payload: UpdateErrorEvent }
  | { type: "dismissed" };

export const initialUpdateUiState: UpdateUiState = {
  info: null,
  phase: "idle",
  percent: null,
  error: null,
};

export function reduceUpdateUiState(
  state: UpdateUiState,
  event: UpdateUiEvent,
): UpdateUiState {
  if (event.type === "dismissed") return initialUpdateUiState;
  if (event.type === "available") {
    return { info: event.payload, phase: "idle", percent: null, error: null };
  }
  if (!state.info || event.payload.version !== state.info.version) return state;
  if (event.type === "failed") {
    return { ...state, phase: "failed", error: event.payload };
  }
  return {
    ...state,
    phase: event.payload.status,
    percent: normalizePercent(event.payload.percent),
    error: null,
  };
}

function normalizePercent(percent: number | null | undefined): number | null {
  if (percent == null || !Number.isFinite(percent)) return null;
  return Math.max(0, Math.min(100, percent));
}
