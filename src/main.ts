import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";

import { api } from "./api";
import { LatencyChart } from "./chart";
import {
  formatDuration,
  formatLatency,
  reasonLabel,
  resolveLanguage,
  stateLabel,
} from "./i18n";
import type {
  AppSettings,
  DashboardSnapshot,
  HistoryResponse,
  LiveTargetStatus,
  QualityState,
  QualityThresholds,
  QualityTransitionEvent,
  Target,
} from "./types";
import "./styles.css";

type Language = "ko" | "en";

const rootElement = document.querySelector<HTMLDivElement>("#app");
if (!rootElement) throw new Error("LNPM root element is missing");
const root: HTMLDivElement = rootElement;

const isPopup = new URLSearchParams(location.search).get("view") === "popup";
let settings: AppSettings = {
  retentionDays: 30,
  notificationsEnabled: true,
  startAtLogin: false,
  language: "auto",
  firstRun: true,
};
let language: Language = resolveLanguage(settings.language);
let dashboard: DashboardSnapshot = { nowMs: Date.now(), paused: false, targets: [] };
let history: HistoryResponse | null = null;
let selectedTargetId: string | null = null;
let chart: LatencyChart | null = null;
let loadGeneration = 0;
let currentRange = { fromMs: Date.now() - 3_600_000, toMs: Date.now() };
let followLive = true;

void bootstrap();

async function bootstrap(): Promise<void> {
  try {
    settings = await api.settings();
    language = resolveLanguage(settings.language);
    dashboard = await api.dashboard();
  } catch (error) {
    console.error(error);
  }

  if (isPopup) await initPopup();
  else await initMain();

  await listen<DashboardSnapshot>("dashboard-updated", (event) => {
    dashboard = event.payload;
    if (isPopup) renderPopupStatus();
    else renderDashboard();
  });
  await listen<QualityTransitionEvent>("quality-transition", (event) => {
    showTransitionToast(event.payload);
    if (isPopup) schedulePopupHide();
  });
  await listen<{ targetId: string | null; message: string }>("monitor-error", (event) => {
    showToast(event.payload.message, "error");
  });
}

async function initMain(): Promise<void> {
  root.className = "app-shell";
  root.innerHTML = `
    <header class="app-header">
      <div class="brand-block">
        <div class="brand-mark" aria-hidden="true"><span></span></div>
        <div><h1>Live Network Ping Monitor</h1><p>${copy("Live network quality", "실시간 네트워크 품질")}</p></div>
      </div>
      <div class="header-actions">
        <button id="follow-live" class="button ghost active">● ${copy("Live", "실시간")}</button>
        <button id="pause-monitoring" class="button ghost"></button>
        <button id="open-settings" class="button icon-button" aria-label="Settings">⚙</button>
      </div>
    </header>
    <div id="update-banner" class="update-banner hidden"></div>
    <main class="workspace">
      <aside class="target-sidebar">
        <div class="sidebar-heading">
          <div><span class="eyebrow">${copy("MONITORS", "모니터")}</span><strong id="target-count">0</strong></div>
          <button id="add-target" class="button add-button" aria-label="Add target">＋</button>
        </div>
        <div id="target-list" class="target-list"></div>
        <div id="empty-targets" class="empty-targets hidden">
          <div class="empty-radar"></div>
          <strong>${copy("No targets yet", "감시 대상이 없습니다")}</strong>
          <p>${copy("Add a host or IP address to begin monitoring.", "호스트명이나 IP 주소를 추가해 측정을 시작하세요.")}</p>
          <button class="button primary" data-action="add-target">${copy("Add target", "대상 추가")}</button>
        </div>
      </aside>
      <section class="dashboard-panel">
        <div class="dashboard-heading">
          <div>
            <span id="selected-state" class="state-pill state-warmingUp"></span>
            <h2 id="selected-name">${copy("Overview", "전체 현황")}</h2>
            <p id="selected-host">—</p>
          </div>
          <div class="range-controls" role="group" aria-label="Chart range">
            <button data-range="3600000" class="range-button active">1H</button>
            <button data-range="86400000" class="range-button">24H</button>
            <button data-range="604800000" class="range-button">7D</button>
            <button data-range="2592000000" class="range-button">30D</button>
            <button id="custom-range" class="range-button">${copy("Custom", "직접 선택")}</button>
          </div>
        </div>
        <div class="chart-card">
          <div id="main-chart" class="chart-host"></div>
          <div id="chart-loading" class="chart-loading hidden">${copy("Loading history…", "기록을 불러오는 중…")}</div>
          <div class="chart-legend" id="chart-legend"></div>
        </div>
        <div class="summary-grid">
          <article class="metric-card"><span>${copy("Average", "평균 지연")}</span><strong id="metric-average">—</strong></article>
          <article class="metric-card"><span>P95</span><strong id="metric-p95">—</strong></article>
          <article class="metric-card warning"><span>${copy("Unstable", "불안정")}</span><strong id="metric-unstable">—</strong><small id="metric-unstable-time"></small></article>
          <article class="metric-card danger"><span>${copy("Disconnected", "연결 끊김")}</span><strong id="metric-disconnected">—</strong><small id="metric-disconnected-time"></small></article>
          <article class="metric-card"><span>${copy("Packet loss", "패킷 손실")}</span><strong id="metric-loss">—</strong></article>
        </div>
      </section>
    </main>
    <dialog id="target-dialog" class="modal"></dialog>
    <dialog id="settings-dialog" class="modal settings-modal"></dialog>
    <dialog id="range-dialog" class="modal compact-modal">
      <form id="range-form">
        <header><h3>${copy("Custom range", "날짜 범위 선택")}</h3><button type="button" class="modal-close">×</button></header>
        <label>${copy("From", "시작")}<input id="range-from" type="datetime-local" required /></label>
        <label>${copy("To", "종료")}<input id="range-to" type="datetime-local" required /></label>
        <footer><button type="button" class="button ghost modal-close">${copy("Cancel", "취소")}</button><button class="button primary">${copy("Apply", "적용")}</button></footer>
      </form>
    </dialog>
    <div id="toast-stack" class="toast-stack" aria-live="polite"></div>
  `;

  bindMainEvents();
  renderDashboard();
  if (dashboard.targets.length > 0) {
    selectedTargetId = dashboard.targets[0].target.id;
    await loadHistory(currentRange.fromMs, currentRange.toMs);
  } else {
    openTargetDialog();
  }
  void checkForUpdates();
  window.setInterval(() => {
    if (followLive && dashboard.targets.length > 0) {
      const toMs = Date.now();
      const duration = currentRange.toMs - currentRange.fromMs;
      void loadHistory(toMs - duration, toMs, false);
    }
  }, 5_000);
}

function bindMainEvents(): void {
  byId("add-target").addEventListener("click", () => openTargetDialog());
  root.querySelector('[data-action="add-target"]')?.addEventListener("click", () => openTargetDialog());
  byId("open-settings").addEventListener("click", () => void openSettingsDialog());
  byId("pause-monitoring").addEventListener("click", () => void api.pause(!dashboard.paused));
  byId("follow-live").addEventListener("click", () => {
    followLive = true;
    byId("follow-live").classList.add("active");
    const duration = currentRange.toMs - currentRange.fromMs;
    const toMs = Date.now();
    void loadHistory(toMs - duration, toMs);
  });
  root.querySelectorAll<HTMLButtonElement>("[data-range]").forEach((button) => {
    button.addEventListener("click", () => {
      const duration = Number(button.dataset.range);
      const toMs = Date.now();
      followLive = true;
      root.querySelectorAll(".range-button").forEach((item) => item.classList.remove("active"));
      button.classList.add("active");
      byId("follow-live").classList.add("active");
      void loadHistory(toMs - duration, toMs);
    });
  });
  byId("custom-range").addEventListener("click", () => {
    const dialog = byId<HTMLDialogElement>("range-dialog");
    byId<HTMLInputElement>("range-from").value = toLocalInput(currentRange.fromMs);
    byId<HTMLInputElement>("range-to").value = toLocalInput(currentRange.toMs);
    dialog.showModal();
  });
  byId<HTMLFormElement>("range-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const fromMs = new Date(byId<HTMLInputElement>("range-from").value).getTime();
    const toMs = new Date(byId<HTMLInputElement>("range-to").value).getTime();
    if (toMs <= fromMs) {
      showToast(copy("End must be after start", "종료 시각은 시작 시각보다 뒤여야 합니다."), "error");
      return;
    }
    followLive = false;
    byId("follow-live").classList.remove("active");
    byId<HTMLDialogElement>("range-dialog").close();
    void loadHistory(fromMs, toMs);
  });
  root.querySelectorAll<HTMLButtonElement>(".modal-close").forEach((button) => {
    button.addEventListener("click", () => button.closest("dialog")?.close());
  });
}

function renderDashboard(): void {
  if (isPopup || !document.querySelector(".workspace")) return;
  byId("target-count").textContent = String(dashboard.targets.length);
  byId("empty-targets").classList.toggle("hidden", dashboard.targets.length > 0);
  byId("target-list").classList.toggle("hidden", dashboard.targets.length === 0);
  byId("pause-monitoring").textContent = dashboard.paused
    ? copy("▶ Resume", "▶ 재개")
    : copy("Ⅱ Pause", "Ⅱ 일시정지");

  if (selectedTargetId && !dashboard.targets.some((item) => item.target.id === selectedTargetId)) {
    selectedTargetId = dashboard.targets[0]?.target.id ?? null;
  }
  const targetList = byId("target-list");
  targetList.innerHTML = dashboard.targets
    .map((item) => {
      const latency = item.latestSample?.latencyMs;
      return `<button class="target-row ${item.target.id === selectedTargetId ? "selected" : ""}" data-target-id="${item.target.id}">
        <span class="status-dot state-${item.state}"></span>
        <span class="target-copy"><strong>${escapeHtml(item.target.name)}</strong><small>${escapeHtml(item.target.host)}</small></span>
        <span class="target-latency">${formatLatency(latency)}</span>
        <span class="target-menu" data-edit-target="${item.target.id}" role="button" aria-label="${copy("Manage target", "대상 관리")}">${copy("Manage", "관리")}</span>
      </button>`;
    })
    .join("");
  targetList.querySelectorAll<HTMLButtonElement>(".target-row").forEach((row) => {
    row.addEventListener("click", (event) => {
      const targetId = row.dataset.targetId ?? null;
      if ((event.target as HTMLElement).closest("[data-edit-target]")) {
        const status = dashboard.targets.find((item) => item.target.id === targetId);
        if (status) openTargetDialog(status.target);
        return;
      }
      selectedTargetId = targetId;
      renderDashboard();
      if (history) chart?.render(history, selectedTargetId);
      renderSummary();
    });
  });

  const selected = selectedStatus();
  byId("selected-name").textContent = selected?.target.name ?? copy("Overview", "전체 현황");
  byId("selected-host").textContent = selected?.target.host ?? "—";
  const statePill = byId("selected-state");
  const state = selected?.state ?? aggregateState();
  statePill.className = `state-pill state-${state}`;
  statePill.textContent = stateLabel(state, language);
  renderSummary();
}

async function loadHistory(fromMs: number, toMs: number, showLoading = true): Promise<void> {
  if (dashboard.targets.length === 0) return;
  const generation = ++loadGeneration;
  currentRange = { fromMs, toMs };
  if (showLoading) byId("chart-loading").classList.remove("hidden");
  try {
    const response = await api.history(
      dashboard.targets.map((item) => item.target.id),
      Math.round(fromMs),
      Math.round(toMs),
      Math.max(600, Math.min(4_000, byId("main-chart").clientWidth * 2)),
    );
    if (generation !== loadGeneration) return;
    history = response;
    chart?.destroy();
    chart = new LatencyChart(byId("main-chart"), {
      selectedTargetId,
      onRangeChanged: (nextFrom, nextTo) => {
        followLive = false;
        byId("follow-live").classList.remove("active");
        void loadHistory(nextFrom, nextTo, false);
      },
    });
    chart.render(response, selectedTargetId);
    renderLegend();
    renderSummary();
  } catch (error) {
    showToast(String(error), "error");
  } finally {
    if (generation === loadGeneration && showLoading) byId("chart-loading").classList.add("hidden");
  }
}

function renderLegend(): void {
  if (!history) return;
  byId("chart-legend").innerHTML = history.series
    .map(
      (series, index) =>
        `<button data-legend-id="${series.target.id}" class="legend-item ${series.target.id === selectedTargetId ? "selected" : ""}"><span style="--series-color:${["#5eead4", "#60a5fa", "#c084fc", "#f472b6", "#facc15"][index % 5]}"></span>${escapeHtml(series.target.name)}</button>`,
    )
    .join("");
  byId("chart-legend").querySelectorAll<HTMLButtonElement>("[data-legend-id]").forEach((button) => {
    button.addEventListener("click", () => {
      selectedTargetId = button.dataset.legendId ?? null;
      renderDashboard();
      chart?.render(history!, selectedTargetId);
      renderLegend();
    });
  });
}

function renderSummary(): void {
  const summary = history?.series.find((series) => series.target.id === selectedTargetId)?.summary;
  setText("metric-average", formatLatency(summary?.averageLatencyMs));
  setText("metric-p95", formatLatency(summary?.p95LatencyMs));
  setText("metric-unstable", summary ? `${summary.unstablePercent.toFixed(2)}%` : "—");
  setText("metric-disconnected", summary ? `${summary.disconnectedPercent.toFixed(2)}%` : "—");
  setText("metric-loss", summary ? `${summary.packetLossPercent.toFixed(2)}%` : "—");
  setText("metric-unstable-time", summary ? formatDuration(summary.unstableMs, language) : "");
  setText(
    "metric-disconnected-time",
    summary ? formatDuration(summary.disconnectedMs, language) : "",
  );
}

function openTargetDialog(existing?: Target): void {
  const dialog = byId<HTMLDialogElement>("target-dialog");
  const target = existing ?? temporaryTarget();
  dialog.innerHTML = `
    <form id="target-form">
      <header><div><span class="eyebrow">${existing ? copy("EDIT MONITOR", "모니터 수정") : copy("NEW MONITOR", "새 모니터")}</span><h3>${existing ? escapeHtml(existing.name) : copy("Add a target", "대상 추가")}</h3></div><button type="button" class="modal-close">×</button></header>
      <input id="target-id" type="hidden" value="${escapeHtml(existing?.id ?? "")}" />
      <div class="form-grid two-columns">
        <label>${copy("Display name", "표시 이름")}<input id="target-name" value="${escapeHtml(target.name)}" required maxlength="40" placeholder="Cloudflare DNS" /></label>
        <label>${copy("Hostname or IP", "호스트명 또는 IP")}<input id="target-host" value="${escapeHtml(target.host)}" required placeholder="1.1.1.1" /></label>
      </div>
      <div class="form-grid three-columns">
        <label>${copy("Address family", "주소 유형")}<select id="target-family"><option value="auto">Auto</option><option value="ipv4">IPv4</option><option value="ipv6">IPv6</option></select></label>
        <label>${copy("Interval", "측정 주기")}<div class="input-suffix"><input id="target-interval" type="number" min="1" max="60" value="${target.intervalMs / 1_000}" /><span>s</span></div></label>
        <label>${copy("Timeout", "타임아웃")}<div class="input-suffix"><input id="target-timeout" type="number" min="250" max="10000" step="250" value="${target.timeoutMs}" /><span>ms</span></div></label>
      </div>
      <fieldset><legend>${copy("Unstable thresholds", "불안정 판정 기준")}</legend><div class="form-grid three-columns">
        <label>${copy("Packet loss", "패킷 손실")}<div class="input-suffix"><input id="threshold-loss" type="number" min="0" max="100" step="0.5" value="${target.thresholds.packetLossPercent}" /><span>%</span></div></label>
        <label>${copy("Jitter", "지터")}<div class="input-suffix"><input id="threshold-jitter" type="number" min="1" max="1000" value="${target.thresholds.jitterMs}" /><span>ms</span></div></label>
        <label>P95 latency<div class="input-suffix"><input id="threshold-p95" type="number" min="1" max="10000" value="${target.thresholds.p95LatencyMs}" /><span>ms</span></div></label>
      </div></fieldset>
      <label class="toggle-row"><input id="target-enabled" type="checkbox" ${target.enabled ? "checked" : ""}/><span><strong>${copy("Monitoring enabled", "모니터링 활성화")}</strong><small>${copy("Collect samples while LNPM is running", "LNPM 실행 중 측정값을 수집합니다")}</small></span></label>
      <div id="target-test-result" class="test-result"></div>
      <footer>
        <div>${existing ? `<button id="delete-target" type="button" class="button danger-text">${copy("Remove monitor", "모니터 삭제")}</button>` : ""}</div>
        <div><button id="test-target" type="button" class="button ghost">${copy("Test ping", "Ping 테스트")}</button><button type="submit" class="button primary">${copy("Save", "저장")}</button></div>
      </footer>
    </form>`;
  byId<HTMLSelectElement>("target-family").value = target.addressFamily;
  dialog.querySelector(".modal-close")?.addEventListener("click", () => dialog.close());
  byId("test-target").addEventListener("click", () => void testTargetForm(target));
  byId<HTMLFormElement>("target-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void saveTargetForm(target, dialog);
  });
  dialog
    .querySelector("#delete-target")
    ?.addEventListener("click", () => void removeTarget(target, dialog));
  dialog.showModal();
}

function readTargetForm(base: Target): Target {
  return {
    ...base,
    id: byId<HTMLInputElement>("target-id").value || base.id,
    name: byId<HTMLInputElement>("target-name").value.trim(),
    host: byId<HTMLInputElement>("target-host").value.trim(),
    addressFamily: byId<HTMLSelectElement>("target-family").value as Target["addressFamily"],
    intervalMs: Number(byId<HTMLInputElement>("target-interval").value) * 1_000,
    timeoutMs: Number(byId<HTMLInputElement>("target-timeout").value),
    enabled: byId<HTMLInputElement>("target-enabled").checked,
    thresholds: {
      ...base.thresholds,
      packetLossPercent: Number(byId<HTMLInputElement>("threshold-loss").value),
      jitterMs: Number(byId<HTMLInputElement>("threshold-jitter").value),
      p95LatencyMs: Number(byId<HTMLInputElement>("threshold-p95").value),
    },
  };
}

async function testTargetForm(base: Target): Promise<void> {
  const result = byId("target-test-result");
  result.className = "test-result visible";
  result.textContent = copy("Testing…", "테스트 중…");
  try {
    const sample = await api.testTarget(readTargetForm(base));
    result.className = `test-result visible ${sample.status === "success" ? "success" : "error"}`;
    result.textContent =
      sample.status === "success"
        ? `${copy("Reply", "응답")} ${formatLatency(sample.latencyMs)} · ${sample.resolvedAddress ?? ""}`
        : `${sample.status}: ${sample.error ?? copy("No response", "응답 없음")}`;
  } catch (error) {
    result.className = "test-result visible error";
    result.textContent = String(error);
  }
}

async function saveTargetForm(base: Target, dialog: HTMLDialogElement): Promise<void> {
  try {
    let target = readTargetForm(base);
    if (!byId<HTMLInputElement>("target-id").value) {
      const created = await api.createTarget(target.name, target.host);
      target = { ...created, ...target, id: created.id, createdAtMs: created.createdAtMs };
    }
    const saved = await api.saveTarget(target);
    selectedTargetId = saved.id;
    dialog.close();
    settings.firstRun = false;
    await api.saveSettings(settings);
    dashboard = await api.dashboard();
    renderDashboard();
    const toMs = Date.now();
    await loadHistory(toMs - (currentRange.toMs - currentRange.fromMs), toMs);
    showToast(copy("Target saved", "대상을 저장했습니다"), "success");
  } catch (error) {
    showToast(String(error), "error");
  }
}

async function removeTarget(target: Target, dialog: HTMLDialogElement): Promise<void> {
  const accepted = confirm(
    copy(
      `Remove ${target.name}? Historical data will be kept.`,
      `${target.name} 대상을 삭제할까요? 과거 기록은 유지됩니다.`,
    ),
  );
  if (!accepted) return;
  await api.archiveTarget(target.id);
  dialog.close();
  dashboard = await api.dashboard();
  selectedTargetId = dashboard.targets[0]?.target.id ?? null;
  renderDashboard();
  if (selectedTargetId) await loadHistory(currentRange.fromMs, currentRange.toMs);
  else {
    chart?.destroy();
    chart = null;
    history = null;
  }
}

async function openSettingsDialog(): Promise<void> {
  const dialog = byId<HTMLDialogElement>("settings-dialog");
  const storage = await api.storageInfo();
  dialog.innerHTML = `
    <form id="settings-form" class="settings-form">
      <header><div><span class="eyebrow">LNPM</span><h3>${copy("Settings", "설정")}</h3></div><button type="button" class="modal-close">×</button></header>
      <div class="settings-scroll-area">
        <section class="settings-section"><h4>${copy("Monitoring", "모니터링")}</h4>
          <label>${copy("Raw sample retention", "원본 보존 기간")}<select id="retention-days"><option value="7">7 days</option><option value="30">30 days</option><option value="90">90 days</option><option value="180">180 days</option><option value="365">365 days</option><option value="unlimited">Unlimited</option></select></label>
          <label class="toggle-row"><input id="notifications-enabled" type="checkbox" ${settings.notificationsEnabled ? "checked" : ""}/><span><strong>${copy("Notifications", "상태 알림")}</strong><small>${copy("Notify on unstable, disconnected and recovered states", "불안정·끊김·복구 상태를 알립니다")}</small></span></label>
          <label class="toggle-row"><input id="start-at-login" type="checkbox" ${settings.startAtLogin ? "checked" : ""}/><span><strong>${copy("Start at login", "로그인 시 자동 시작")}</strong><small>${copy("Start LNPM in the tray", "LNPM을 트레이에서 자동 실행합니다")}</small></span></label>
        </section>
        <section class="settings-section"><h4>${copy("Appearance", "표시")}</h4><label>${copy("Language", "언어")}<select id="language"><option value="auto">System default</option><option value="ko">한국어</option><option value="en">English</option></select></label></section>
        <section class="settings-section data-section"><h4>${copy("Data", "데이터")}</h4><div><span>${formatBytes(storage.databaseSizeBytes)}</span><small>${escapeHtml(storage.databasePath)}</small></div><div class="inline-actions"><button id="open-data" type="button" class="button ghost">${copy("Open folder", "폴더 열기")}</button><button id="backup-data" type="button" class="button ghost">${copy("Create backup", "백업 생성")}</button><button id="cleanup-data" type="button" class="button ghost">${copy("Clean now", "지금 정리")}</button></div></section>
      </div>
      <footer><span>v${await getVersion()}</span><div><button type="button" class="button ghost modal-close">${copy("Cancel", "취소")}</button><button class="button primary">${copy("Save", "저장")}</button></div></footer>
    </form>`;
  byId<HTMLSelectElement>("retention-days").value = settings.retentionDays?.toString() ?? "unlimited";
  byId<HTMLSelectElement>("language").value = settings.language;
  dialog.querySelectorAll(".modal-close").forEach((button) =>
    button.addEventListener("click", () => dialog.close()),
  );
  byId("open-data").addEventListener("click", () => void openPath(storage.dataDirectory));
  byId("backup-data").addEventListener("click", async () => {
    const path = await api.backup();
    showToast(copy(`Backup created: ${path}`, `백업 생성: ${path}`), "success");
  });
  byId("cleanup-data").addEventListener("click", async () => {
    const deleted = await api.cleanup();
    showToast(copy(`Removed ${deleted} raw samples`, `원본 ${deleted}개를 정리했습니다`), "success");
  });
  byId<HTMLFormElement>("settings-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const retention = byId<HTMLSelectElement>("retention-days").value;
    settings = await api.saveSettings({
      ...settings,
      retentionDays: retention === "unlimited" ? null : Number(retention),
      notificationsEnabled: byId<HTMLInputElement>("notifications-enabled").checked,
      startAtLogin: byId<HTMLInputElement>("start-at-login").checked,
      language: byId<HTMLSelectElement>("language").value as AppSettings["language"],
      firstRun: false,
    });
    dialog.close();
    if (language !== resolveLanguage(settings.language)) location.reload();
    else showToast(copy("Settings saved", "설정을 저장했습니다"), "success");
  });
  dialog.showModal();
}

async function initPopup(): Promise<void> {
  root.className = "popup-shell";
  root.innerHTML = `
    <section class="popup-card">
      <header><div class="brand-mark small"><span></span></div><div><strong>LNPM</strong><span id="popup-overall"></span></div><button id="popup-close" aria-label="Close">×</button></header>
      <div id="popup-alert" class="popup-alert hidden"></div>
      <div id="popup-targets" class="popup-targets"></div>
      <div id="popup-chart" class="popup-chart"></div>
      <footer><button id="popup-pause" class="button ghost"></button><button id="popup-details" class="button primary">${copy("Details", "상세 보기")} ↗</button></footer>
    </section>
    <div id="toast-stack" class="toast-stack"></div>`;
  byId("popup-close").addEventListener("click", () => void api.hidePopup());
  byId("popup-details").addEventListener("click", () => void api.showMain());
  byId("popup-pause").addEventListener("click", () => void api.pause(!dashboard.paused));
  renderPopupStatus();
  await loadPopupHistory();
  window.setInterval(() => void loadPopupHistory(), 5_000);
}

function renderPopupStatus(): void {
  if (!isPopup || !document.querySelector(".popup-card")) return;
  const overall = aggregateState();
  const overallElement = byId("popup-overall");
  overallElement.className = `state-text state-${overall}`;
  overallElement.textContent = stateLabel(overall, language);
  byId("popup-pause").textContent = dashboard.paused
    ? copy("Resume", "재개")
    : copy("Pause", "일시정지");
  byId("popup-targets").innerHTML = dashboard.targets.length
    ? dashboard.targets
        .map(
          (item) => `<div class="popup-target-row"><span class="status-dot state-${item.state}"></span><strong>${escapeHtml(item.target.name)}</strong><small>${stateLabel(item.state, language)}</small><span>${formatLatency(item.latestSample?.latencyMs)}</span></div>`,
        )
        .join("")
    : `<div class="popup-empty">${copy("Open details to add a monitor", "상세 화면에서 모니터를 추가하세요")}</div>`;
}

async function loadPopupHistory(): Promise<void> {
  if (dashboard.targets.length === 0) return;
  const toMs = Date.now();
  try {
    const response = await api.history(
      dashboard.targets.map((item) => item.target.id),
      toMs - 5 * 60_000,
      toMs,
      600,
    );
    chart?.destroy();
    chart = new LatencyChart(byId("popup-chart"), { compact: true });
    chart.render(response);
  } catch (error) {
    console.error(error);
  }
}

function showTransitionToast(event: QualityTransitionEvent): void {
  const message = `${event.target.name} · ${stateLabel(event.transition.to, language)}`;
  showToast(message, event.transition.to === "stable" ? "success" : event.transition.to);
  if (isPopup) {
    const alert = byId("popup-alert");
    alert.className = `popup-alert state-${event.transition.to}`;
    alert.innerHTML = `<strong>${escapeHtml(message)}</strong><small>${event.transition.reasons.map((reason) => reasonLabel(reason, language)).join(" · ")}</small>`;
  }
}

let popupHideTimer = 0;
function schedulePopupHide(): void {
  window.clearTimeout(popupHideTimer);
  popupHideTimer = window.setTimeout(() => void api.hidePopup(), 5_000);
  root.addEventListener("mouseenter", () => window.clearTimeout(popupHideTimer), { once: true });
  root.addEventListener("mouseleave", () => schedulePopupHide(), { once: true });
}

async function checkForUpdates(): Promise<void> {
  const lastCheck = Number(localStorage.getItem("lnpm-update-check") ?? 0);
  if (Date.now() - lastCheck < 86_400_000) return;
  localStorage.setItem("lnpm-update-check", String(Date.now()));
  try {
    const response = await fetch("https://api.github.com/repos/xxsLuna/LNPM/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) return;
    const release = (await response.json()) as { tag_name: string; html_url: string };
    const current = await getVersion();
    if (!isNewerVersion(release.tag_name, current)) return;
    const banner = byId("update-banner");
    banner.classList.remove("hidden");
    banner.innerHTML = `<span>${copy(`LNPM ${release.tag_name} is available.`, `LNPM ${release.tag_name} 버전이 있습니다.`)}</span><button class="button ghost">${copy("View release", "릴리스 보기")}</button>`;
    banner.querySelector("button")?.addEventListener("click", () => void openUrl(release.html_url));
  } catch (error) {
    console.debug("Update check failed", error);
  }
}

function temporaryTarget(): Target {
  const thresholds: QualityThresholds = {
    windowSeconds: 60,
    minimumSamples: 10,
    packetLossPercent: 5,
    jitterMs: 30,
    p95LatencyMs: 150,
    unstableForSeconds: 10,
    stableForSeconds: 30,
    outageFailures: 5,
    recoverySuccesses: 3,
  };
  return {
    id: crypto.randomUUID(),
    name: "",
    host: "",
    enabled: true,
    addressFamily: "auto",
    intervalMs: 1_000,
    timeoutMs: 1_000,
    thresholds,
    createdAtMs: Date.now(),
    archivedAtMs: null,
  };
}

function selectedStatus(): LiveTargetStatus | undefined {
  return dashboard.targets.find((item) => item.target.id === selectedTargetId);
}

function aggregateState(): QualityState {
  if (dashboard.paused || dashboard.targets.length === 0) return "paused";
  const priority: Record<QualityState, number> = {
    stable: 0,
    paused: 1,
    warmingUp: 2,
    unobserved: 2,
    unstable: 3,
    disconnected: 4,
    error: 4,
  };
  return dashboard.targets.reduce(
    (worst, target) => (priority[target.state] > priority[worst] ? target.state : worst),
    "stable" as QualityState,
  );
}

function showToast(message: string, kind: string): void {
  const stack = document.querySelector<HTMLDivElement>("#toast-stack");
  if (!stack) return;
  const toast = document.createElement("div");
  toast.className = `toast toast-${kind}`;
  toast.textContent = message;
  stack.append(toast);
  window.setTimeout(() => toast.classList.add("visible"), 10);
  window.setTimeout(() => {
    toast.classList.remove("visible");
    window.setTimeout(() => toast.remove(), 240);
  }, 4_500);
}

function copy(en: string, ko: string): string {
  return language === "ko" ? ko : en;
}

function byId<T extends HTMLElement = HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) throw new Error(`Missing element #${id}`);
  return element as T;
}

function setText(id: string, value: string): void {
  const element = document.getElementById(id);
  if (element) element.textContent = value;
}

function toLocalInput(timestampMs: number): string {
  const date = new Date(timestampMs - new Date(timestampMs).getTimezoneOffset() * 60_000);
  return date.toISOString().slice(0, 16);
}

function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  if (bytes < 1_073_741_824) return `${(bytes / 1_048_576).toFixed(1)} MB`;
  return `${(bytes / 1_073_741_824).toFixed(2)} GB`;
}

function isNewerVersion(tag: string, current: string): boolean {
  const next = tag.replace(/^v/, "").split("-")[0].split(".").map(Number);
  const active = current.split("-")[0].split(".").map(Number);
  for (let index = 0; index < 3; index += 1) {
    if ((next[index] ?? 0) > (active[index] ?? 0)) return true;
    if ((next[index] ?? 0) < (active[index] ?? 0)) return false;
  }
  return false;
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
