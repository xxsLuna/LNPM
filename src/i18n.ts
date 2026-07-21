import type { ProbeStatus, QualityReason, QualityState, UserErrorPayload } from "./types";
import en from "./locales/en.json";
import ja from "./locales/ja.json";
import ko from "./locales/ko.json";
import zhCN from "./locales/zh-CN.json";
import zhTW from "./locales/zh-TW.json";

export type Language = "en" | "ko" | "ja" | "zh-CN" | "zh-TW";
export type LanguagePreference = "auto" | Language;
export type MessageKey = keyof typeof en;
export type MessageParams = Record<string, string | number>;

type Catalog = Record<MessageKey, string>;

const catalogs: Record<Language, Catalog> = {
  en,
  ko,
  ja,
  "zh-CN": zhCN,
  "zh-TW": zhTW,
};

const errorKeys: Record<string, MessageKey> = {
  unknown: "error.unknown",
  hostRequired: "error.hostRequired",
  invalidHost: "error.invalidHost",
  intervalRange: "error.intervalRange",
  timeoutRange: "error.timeoutRange",
  timeoutInterval: "error.timeoutInterval",
  targetNotFound: "error.targetNotFound",
  storage: "error.storage",
  filesystem: "error.filesystem",
  serialization: "error.serialization",
  invalidData: "error.invalidData",
  autostart: "error.autostart",
  monitoring: "error.monitoring",
  updateCheck: "error.updateCheck",
  updateDownload: "error.updateDownload",
  updateInstall: "error.updateInstall",
  updateSignature: "error.updateSignature",
  updateBusy: "error.updateBusy",
  updateMissing: "error.updateMissing",
};

let activeLanguage: Language = "en";

export function setLanguage(language: Language): void {
  activeLanguage = language;
}

export function getLanguage(): Language {
  return activeLanguage;
}

export function resolveLanguage(
  preference: LanguagePreference = "auto",
  systemLocales: readonly string[] = browserLocales(),
): Language {
  if (preference !== "auto") return preference;
  for (const locale of systemLocales) {
    const resolved = resolveSystemLocale(locale);
    if (resolved) return resolved;
  }
  return "en";
}

export function resolveSystemLocale(locale: string): Language | null {
  const normalized = locale.trim().replace(/_/g, "-").toLowerCase();
  if (normalized === "ko" || normalized.startsWith("ko-")) return "ko";
  if (normalized === "ja" || normalized.startsWith("ja-")) return "ja";
  if (normalized === "en" || normalized.startsWith("en-")) return "en";
  if (normalized === "zh" || normalized.startsWith("zh-")) {
    const parts = normalized.split("-");
    return parts.includes("hant") || ["tw", "hk", "mo"].some((part) => parts.includes(part))
      ? "zh-TW"
      : "zh-CN";
  }
  return null;
}

export function t(
  key: MessageKey,
  params: MessageParams = {},
  language: Language = activeLanguage,
): string {
  const template = catalogs[language][key] ?? catalogs.en[key];
  return template.replace(/\{([A-Za-z][A-Za-z0-9]*)\}/g, (placeholder, name: string) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : placeholder,
  );
}

export function stateLabel(state: QualityState, language = activeLanguage): string {
  return t(`state.${state}` as MessageKey, {}, language);
}

export function reasonLabel(reason: QualityReason, language = activeLanguage): string {
  return t(`reason.${reason}` as MessageKey, {}, language);
}

export function probeStatusLabel(status: ProbeStatus, language = activeLanguage): string {
  return t(`probe.${status}` as MessageKey, {}, language);
}

export function formatDuration(ms: number, language = activeLanguage): string {
  const seconds = Math.max(0, Math.round(ms / 1_000));
  if (seconds < 60) return formatUnit(seconds, "duration.second", language);
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return formatUnit(minutes, "duration.minute", language);
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  if (hours < 24) {
    return [
      formatUnit(hours, "duration.hour", language),
      formatUnit(restMinutes, "duration.minute", language),
    ].join(" ");
  }
  const days = Math.floor(hours / 24);
  return [
    formatUnit(days, "duration.day", language),
    formatUnit(hours % 24, "duration.hour", language),
  ].join(" ");
}

export function formatLatency(
  value: number | null | undefined,
  language = activeLanguage,
): string {
  if (value == null) return "—";
  return `${new Intl.NumberFormat(language, {
    maximumFractionDigits: value < 10 ? 1 : 0,
    minimumFractionDigits: value < 10 ? 1 : 0,
  }).format(value)} ms`;
}

export function formatPercent(value: number | null | undefined, language = activeLanguage): string {
  if (value == null) return "—";
  return `${new Intl.NumberFormat(language, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(value)}%`;
}

export function formatDateTime(timestampMs: number, language = activeLanguage): string {
  return new Intl.DateTimeFormat(language, {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(new Date(timestampMs));
}

export function formatBytes(bytes: number, language = activeLanguage): string {
  if (bytes < 1_024) return `${formatNumber(bytes, language, 0)} B`;
  if (bytes < 1_048_576) return `${formatNumber(bytes / 1_024, language, 1)} KB`;
  if (bytes < 1_073_741_824) return `${formatNumber(bytes / 1_048_576, language, 1)} MB`;
  return `${formatNumber(bytes / 1_073_741_824, language, 2)} GB`;
}

export function formatError(error: unknown, language = activeLanguage): string {
  const payload = normalizeError(error);
  const summary = t(errorKeys[payload.code] ?? "error.unknown", {}, language);
  const detail = payload.detail?.trim();
  return detail && detail !== summary
    ? t("error.withDetail", { summary, detail }, language)
    : summary;
}

export function catalogEntries(language: Language): Readonly<Record<string, string>> {
  return catalogs[language];
}

export function placeholders(value: string): string[] {
  return Array.from(value.matchAll(/\{([A-Za-z][A-Za-z0-9]*)\}/g), (match) => match[1]).sort();
}

function browserLocales(): readonly string[] {
  if (typeof navigator === "undefined") return [];
  return navigator.languages.length > 0 ? navigator.languages : [navigator.language];
}

function formatUnit(value: number, key: MessageKey, language: Language): string {
  return `${formatNumber(value, language, 0)}${t(key, {}, language)}`;
}

function formatNumber(value: number, language: Language, maximumFractionDigits: number): string {
  return new Intl.NumberFormat(language, { maximumFractionDigits }).format(value);
}

export function normalizeError(error: unknown): UserErrorPayload {
  if (isErrorPayload(error)) return error;
  if (error instanceof Error) return { code: "unknown", detail: error.message };
  if (typeof error === "string") {
    try {
      const parsed: unknown = JSON.parse(error);
      if (isErrorPayload(parsed)) return parsed;
    } catch {
      // The original string is useful diagnostic detail.
    }
    return { code: "unknown", detail: error };
  }
  return { code: "unknown", detail: error == null ? null : String(error) };
}

function isErrorPayload(value: unknown): value is UserErrorPayload {
  if (typeof value !== "object" || value == null || !("code" in value)) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.code === "string" &&
    (candidate.detail == null || typeof candidate.detail === "string")
  );
}
