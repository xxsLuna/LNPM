import { describe, expect, it } from "vitest";

import {
  catalogEntries,
  formatBytes,
  formatDateTime,
  formatDuration,
  formatError,
  formatLatency,
  formatPercent,
  placeholders,
  resolveLanguage,
  resolveSystemLocale,
  t,
  type Language,
} from "./i18n";

const languages: Language[] = ["en", "ko", "ja", "zh-CN", "zh-TW"];

describe("translation catalogs", () => {
  it("uses the exact English key set and placeholders in every locale", () => {
    const english = catalogEntries("en");
    const expectedKeys = Object.keys(english).sort();

    for (const language of languages) {
      const catalog = catalogEntries(language);
      expect(Object.keys(catalog).sort()).toEqual(expectedKeys);
      for (const key of expectedKeys) {
        expect(placeholders(catalog[key])).toEqual(placeholders(english[key]));
      }
    }
  });

  it("interpolates named values", () => {
    expect(t("toast.removeConfirm", { name: "Gateway" }, "ko")).toContain("Gateway");
    expect(t("notification.recovered", { name: "DNS" }, "ja")).toBe(
      "DNS の接続が復旧しました",
    );
  });
});

describe("locale resolution", () => {
  it.each([
    ["ko_KR", "ko"],
    ["ja-JP", "ja"],
    ["zh-Hans-CN", "zh-CN"],
    ["zh-SG", "zh-CN"],
    ["zh-Hant", "zh-TW"],
    ["zh-HK", "zh-TW"],
    ["en-US", "en"],
    ["fr-FR", null],
  ])("maps %s to %s", (locale, expected) => {
    expect(resolveSystemLocale(locale)).toBe(expected);
  });

  it("uses the first supported system locale and falls back to English", () => {
    expect(resolveLanguage("auto", ["fr-FR", "ja-JP"])).toBe("ja");
    expect(resolveLanguage("auto", ["fr-FR"])).toBe("en");
    expect(resolveLanguage("zh-TW", ["en-US"])).toBe("zh-TW");
  });
});

describe("localized formatting", () => {
  it("formats compact durations and latency", () => {
    expect(formatDuration(3_661_000, "en")).toBe("1h 1m");
    expect(formatDuration(3_661_000, "ko")).toBe("1시간 1분");
    expect(formatLatency(3.75, "en")).toBe("3.8 ms");
    expect(formatPercent(12.5, "en")).toBe("12.50%");
    expect(formatBytes(1_536, "ja")).toBe("1.5 KB");
    expect(formatDateTime(Date.UTC(2026, 6, 21), "zh-CN")).toContain("2026");
  });

  it("renders translated error summaries with raw diagnostic detail", () => {
    expect(formatError({ code: "targetNotFound", detail: "target not found: abc" }, "ko")).toBe(
      "모니터를 찾을 수 없습니다\ntarget not found: abc",
    );
  });
});
