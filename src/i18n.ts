import type { QualityReason, QualityState } from "./types";

type Language = "ko" | "en";

const messages = {
  en: {
    stable: "Stable",
    unstable: "Unstable",
    disconnected: "Disconnected",
    warmingUp: "Warming up",
    paused: "Paused",
    unobserved: "No data",
    error: "Error",
    packetLoss: "Packet loss",
    jitter: "High jitter",
    highLatency: "High latency",
    consecutiveFailures: "No response",
    configuration: "Configuration error",
  },
  ko: {
    stable: "안정",
    unstable: "불안정",
    disconnected: "연결 끊김",
    warmingUp: "측정 준비 중",
    paused: "일시정지",
    unobserved: "측정 없음",
    error: "오류",
    packetLoss: "패킷 손실",
    jitter: "높은 지터",
    highLatency: "높은 지연",
    consecutiveFailures: "응답 없음",
    configuration: "설정 오류",
  },
} as const;

export function resolveLanguage(preference: "auto" | "ko" | "en" = "auto"): Language {
  if (preference !== "auto") return preference;
  return navigator.language.toLowerCase().startsWith("ko") ? "ko" : "en";
}

export function stateLabel(state: QualityState, language: Language): string {
  return messages[language][state];
}

export function reasonLabel(reason: QualityReason, language: Language): string {
  return messages[language][reason];
}

export function formatDuration(ms: number, language: Language): string {
  const seconds = Math.max(0, Math.round(ms / 1_000));
  if (seconds < 60) return language === "ko" ? `${seconds}초` : `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return language === "ko" ? `${minutes}분` : `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  if (hours < 24) {
    return language === "ko"
      ? `${hours}시간 ${restMinutes}분`
      : `${hours}h ${restMinutes}m`;
  }
  const days = Math.floor(hours / 24);
  return language === "ko" ? `${days}일 ${hours % 24}시간` : `${days}d ${hours % 24}h`;
}

export function formatLatency(value: number | null | undefined): string {
  return value == null ? "—" : `${value < 10 ? value.toFixed(1) : Math.round(value)} ms`;
}
