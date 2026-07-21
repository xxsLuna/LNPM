import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";

import { api } from "./api";
import { LatencyChart } from "./chart";
import {
  formatBytes,
  formatDuration,
  formatError,
  formatLatency,
  formatPercent,
  normalizeError,
  probeStatusLabel,
  reasonLabel,
  resolveLanguage,
  setLanguage,
  stateLabel,
  t,
  type Language,
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
  UserErrorPayload,
  UpdateErrorEvent,
  UpdateInfo,
  UpdateProgressEvent,
} from "./types";
import {
  initialUpdateUiState,
  reduceUpdateUiState,
  type UpdateUiState,
} from "./update-state";
import "./styles.css";

const rootElement = document.querySelector<HTMLDivElement>("#app");
if (!rootElement) throw new Error("LNPM root element is missing");
const root: HTMLDivElement = rootElement;
const logoUrl = new URL("../docs/assets/lnpm-logo.png", import.meta.url).href;

const isPopup = new URLSearchParams(location.search).get("view") === "popup";
let settings: AppSettings = {
  retentionDays: 30,
  notificationsEnabled: true,
  startAtLogin: false,
  language: "auto",
  firstRun: true,
  updateDeferredVersion: null,
  updateDeferredUntilMs: null,
  skippedUpdateVersion: null,
};
let language: Language = resolveLanguage(settings.language);
let dashboard: DashboardSnapshot = { nowMs: Date.now(), paused: false, targets: [] };
let history: HistoryResponse | null = null;
let selectedTargetId: string | null = null;
let chart: LatencyChart | null = null;
let loadGeneration = 0;
let currentRange = { fromMs: Date.now() - 3_600_000, toMs: Date.now() };
let followLive = true;
let updateUiState: UpdateUiState = initialUpdateUiState;
let currentAppVersion: string | null = null;

void bootstrap();

async function bootstrap(): Promise<void> {
  try {
    settings = await api.settings();
    language = resolveLanguage(settings.language);
    setLanguage(language);
    applyDocumentLanguage();
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
  await listen<{ targetId: string | null } & UserErrorPayload>("monitor-error", (event) => {
    showToast(formatError(event.payload), "error");
  });
  await listen<AppSettings>("settings-updated", (event) => {
    const nextLanguage = resolveLanguage(event.payload.language);
    settings = event.payload;
    if (nextLanguage === language) return;
    preserveViewState();
    window.setTimeout(() => location.reload(), 50);
  });
  await listen<UpdateInfo>("update-available", (event) => {
    void showAvailableUpdate(event.payload);
  });
  await listen<UpdateProgressEvent>("update-progress", (event) => {
    updateUiState = reduceUpdateUiState(updateUiState, {
      type: "progress",
      payload: event.payload,
    });
    renderUpdateDialog();
  });
  await listen<UpdateErrorEvent>("update-error", (event) => {
    updateUiState = reduceUpdateUiState(updateUiState, {
      type: "failed",
      payload: event.payload,
    });
    renderUpdateDialog();
  });
  if (!isPopup) {
    const pendingUpdate = await api.pendingUpdate().catch(() => null);
    if (pendingUpdate) await showAvailableUpdate(pendingUpdate);
  }
}

async function initMain(): Promise<void> {
  root.className = "app-shell";
  root.innerHTML = `
    <header class="app-header">
      <div class="brand-block">
        <div class="brand-mark" aria-hidden="true"><img src="${logoUrl}" alt="" /></div>
        <div><h1>${t("app.title")}</h1><p>${t("app.subtitle")}</p></div>
      </div>
      <div class="header-actions">
        <button id="follow-live" class="button ghost active">● ${t("action.live")}</button>
        <button id="pause-monitoring" class="button ghost"></button>
        <button id="open-settings" class="button icon-button" aria-label="${t("action.settings")}">⚙</button>
      </div>
    </header>
    <main class="workspace">
      <aside class="target-sidebar">
        <div class="sidebar-heading">
          <div><span class="eyebrow">${t("section.monitors")}</span><strong id="target-count">0</strong></div>
          <button id="add-target" class="button add-button" aria-label="${t("action.addTarget")}">＋</button>
        </div>
        <div id="target-list" class="target-list"></div>
        <div id="empty-targets" class="empty-targets hidden">
          <div class="empty-radar"></div>
          <strong>${t("empty.targetsTitle")}</strong>
          <p>${t("empty.targetsDescription")}</p>
          <button class="button primary" data-action="add-target">${t("action.addTarget")}</button>
        </div>
      </aside>
      <section class="dashboard-panel">
        <div class="dashboard-heading">
          <div>
            <span id="selected-state" class="state-pill state-warmingUp"></span>
            <h2 id="selected-name">${t("dashboard.overview")}</h2>
            <p id="selected-host">—</p>
          </div>
          <div class="range-controls" role="group" aria-label="${t("dashboard.chartRange")}">
            <button data-range="3600000" class="range-button active">1H</button>
            <button data-range="86400000" class="range-button">24H</button>
            <button data-range="604800000" class="range-button">7D</button>
            <button data-range="2592000000" class="range-button">30D</button>
            <button id="custom-range" class="range-button">${t("action.custom")}</button>
          </div>
        </div>
        <div class="chart-card">
          <div id="main-chart" class="chart-host"></div>
          <div id="chart-loading" class="chart-loading hidden">${t("dashboard.loadingHistory")}</div>
          <div class="chart-legend" id="chart-legend"></div>
        </div>
        <div class="summary-grid">
          <article class="metric-card"><span>${t("dashboard.average")}</span><strong id="metric-average">—</strong></article>
          <article class="metric-card"><span>${t("dashboard.p95Latency")}</span><strong id="metric-p95">—</strong></article>
          <article class="metric-card warning"><span>${t("dashboard.unstable")}</span><strong id="metric-unstable">—</strong><small id="metric-unstable-time"></small></article>
          <article class="metric-card danger"><span>${t("dashboard.disconnected")}</span><strong id="metric-disconnected">—</strong><small id="metric-disconnected-time"></small></article>
          <article class="metric-card"><span>${t("dashboard.packetLoss")}</span><strong id="metric-loss">—</strong></article>
        </div>
      </section>
    </main>
    <dialog id="target-dialog" class="modal"></dialog>
    <dialog id="settings-dialog" class="modal settings-modal"></dialog>
    <dialog id="update-dialog" class="modal update-modal"></dialog>
    <dialog id="range-dialog" class="modal compact-modal">
      <form id="range-form">
        <header><h3>${t("dashboard.customRange")}</h3><button type="button" class="modal-close" aria-label="${t("action.close")}">×</button></header>
        <label>${t("dashboard.from")}<input id="range-from" type="datetime-local" required /></label>
        <label>${t("dashboard.to")}<input id="range-to" type="datetime-local" required /></label>
        <footer><button type="button" class="button ghost modal-close">${t("action.cancel")}</button><button class="button primary">${t("action.apply")}</button></footer>
      </form>
    </dialog>
    <div id="toast-stack" class="toast-stack" aria-live="polite"></div>
  `;

  bindMainEvents();
  restoreViewState();
  if (dashboard.targets.length > 0) {
    if (!dashboard.targets.some((item) => item.target.id === selectedTargetId)) {
      selectedTargetId = dashboard.targets[0].target.id;
    }
    renderDashboard();
    await loadHistory(currentRange.fromMs, currentRange.toMs);
  } else {
    renderDashboard();
    openTargetDialog();
  }
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
  byId("open-settings").addEventListener("click", () => {
    void openSettingsDialog().catch((error) => showToast(formatError(error), "error"));
  });
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
      showToast(t("toast.endAfterStart"), "error");
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
  byId<HTMLDialogElement>("update-dialog").addEventListener("cancel", (event) => {
    event.preventDefault();
    if (!isUpdateBusy()) void deferCurrentUpdate();
  });
  window.addEventListener("beforeunload", (event) => {
    if (!isUpdateBusy()) return;
    event.preventDefault();
    event.returnValue = "";
  });
}

function renderDashboard(): void {
  if (isPopup || !document.querySelector(".workspace")) return;
  byId("target-count").textContent = String(dashboard.targets.length);
  byId("empty-targets").classList.toggle("hidden", dashboard.targets.length > 0);
  byId("target-list").classList.toggle("hidden", dashboard.targets.length === 0);
  byId("pause-monitoring").textContent = dashboard.paused
    ? `▶ ${t("action.resume")}`
    : `Ⅱ ${t("action.pause")}`;

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
        <span class="target-menu" data-edit-target="${item.target.id}" role="button" aria-label="${t("action.manageTarget")}">${t("action.manage")}</span>
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
  byId("selected-name").textContent = selected?.target.name ?? t("dashboard.overview");
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
    showToast(formatError(error), "error");
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
  setText("metric-unstable", formatPercent(summary?.unstablePercent));
  setText("metric-disconnected", formatPercent(summary?.disconnectedPercent));
  setText("metric-loss", formatPercent(summary?.packetLossPercent));
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
      <header><div><span class="eyebrow">${existing ? t("target.editMonitor") : t("target.newMonitor")}</span><h3>${existing ? escapeHtml(existing.name) : t("action.addTarget")}</h3></div><button type="button" class="modal-close" aria-label="${t("action.close")}">×</button></header>
      <input id="target-id" type="hidden" value="${escapeHtml(existing?.id ?? "")}" />
      <div class="form-grid two-columns">
        <label>${t("target.displayName")}<input id="target-name" value="${escapeHtml(target.name)}" required maxlength="40" placeholder="Cloudflare DNS" /></label>
        <label>${t("target.hostname")}<input id="target-host" value="${escapeHtml(target.host)}" required placeholder="1.1.1.1" /></label>
      </div>
      <div class="form-grid three-columns">
        <label>${t("target.addressFamily")}<select id="target-family"><option value="auto">${t("target.addressAuto")}</option><option value="ipv4">IPv4</option><option value="ipv6">IPv6</option></select></label>
        <label>${t("target.interval")}<div class="input-suffix"><input id="target-interval" type="number" min="1" max="60" value="${target.intervalMs / 1_000}" /><span>s</span></div></label>
        <label>${t("target.timeout")}<div class="input-suffix"><input id="target-timeout" type="number" min="250" max="10000" step="250" value="${target.timeoutMs}" /><span>ms</span></div></label>
      </div>
      <fieldset><legend>${t("target.unstableThresholds")}</legend><div class="form-grid three-columns">
        <label>${t("dashboard.packetLoss")}<div class="input-suffix"><input id="threshold-loss" type="number" min="0" max="100" step="0.5" value="${target.thresholds.packetLossPercent}" /><span>%</span></div></label>
        <label>${t("target.jitter")}<div class="input-suffix"><input id="threshold-jitter" type="number" min="1" max="1000" value="${target.thresholds.jitterMs}" /><span>ms</span></div></label>
        <label>${t("dashboard.p95Latency")}<div class="input-suffix"><input id="threshold-p95" type="number" min="1" max="10000" value="${target.thresholds.p95LatencyMs}" /><span>ms</span></div></label>
      </div></fieldset>
      <label class="toggle-row"><input id="target-enabled" type="checkbox" ${target.enabled ? "checked" : ""}/><span><strong>${t("target.monitoringEnabled")}</strong><small>${t("target.collectSamples")}</small></span></label>
      <div id="target-test-result" class="test-result"></div>
      <footer>
        <div>${existing ? `<button id="delete-target" type="button" class="button danger-text">${t("action.removeMonitor")}</button>` : ""}</div>
        <div><button id="test-target" type="button" class="button ghost">${t("action.testPing")}</button><button type="submit" class="button primary">${t("action.save")}</button></div>
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
  result.textContent = t("test.testing");
  try {
    const sample = await api.testTarget(readTargetForm(base));
    result.className = `test-result visible ${sample.status === "success" ? "success" : "error"}`;
    result.textContent =
      sample.status === "success"
        ? `${t("test.reply")} ${formatLatency(sample.latencyMs)} · ${sample.resolvedAddress ?? ""}`
        : `${probeStatusLabel(sample.status)}${sample.error ? `\n${sample.error}` : ` · ${t("test.noResponse")}`}`;
  } catch (error) {
    result.className = "test-result visible error";
    result.textContent = formatError(error);
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
    showToast(t("toast.targetSaved"), "success");
  } catch (error) {
    showToast(formatError(error), "error");
  }
}

async function removeTarget(target: Target, dialog: HTMLDialogElement): Promise<void> {
  const accepted = confirm(t("toast.removeConfirm", { name: target.name }));
  if (!accepted) return;
  try {
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
  } catch (error) {
    showToast(formatError(error), "error");
  }
}

async function openSettingsDialog(): Promise<void> {
  const dialog = byId<HTMLDialogElement>("settings-dialog");
  const storage = await api.storageInfo();
  const retentionOptions = [7, 30, 90, 180, 365]
    .map((days) => `<option value="${days}">${t("settings.days", { count: days })}</option>`)
    .join("");
  dialog.innerHTML = `
    <form id="settings-form" class="settings-form">
      <header><div><span class="eyebrow">LNPM</span><h3>${t("action.settings")}</h3></div><button type="button" class="modal-close" aria-label="${t("action.close")}">×</button></header>
      <div class="settings-scroll-area">
        <section class="settings-section"><h4>${t("section.monitoring")}</h4>
          <label>${t("settings.rawRetention")}<select id="retention-days">${retentionOptions}<option value="unlimited">${t("settings.unlimited")}</option></select></label>
          <label class="toggle-row"><input id="notifications-enabled" type="checkbox" ${settings.notificationsEnabled ? "checked" : ""}/><span><strong>${t("settings.notifications")}</strong><small>${t("settings.notificationsDescription")}</small></span></label>
          <label class="toggle-row"><input id="start-at-login" type="checkbox" ${settings.startAtLogin ? "checked" : ""}/><span><strong>${t("settings.startAtLogin")}</strong><small>${t("settings.startAtLoginDescription")}</small></span></label>
        </section>
        <section class="settings-section"><h4>${t("section.appearance")}</h4><label>${t("settings.language")}<select id="language"><option value="auto">${t("settings.systemDefault")}</option><option value="en">English</option><option value="ko">한국어</option><option value="ja">日本語</option><option value="zh-CN">简体中文</option><option value="zh-TW">繁體中文</option></select></label></section>
        <section class="settings-section data-section"><h4>${t("section.data")}</h4><div><span>${formatBytes(storage.databaseSizeBytes)}</span><small>${escapeHtml(storage.databasePath)}</small></div><div class="inline-actions"><button id="open-data" type="button" class="button ghost">${t("action.openFolder")}</button><button id="backup-data" type="button" class="button ghost">${t("action.createBackup")}</button><button id="cleanup-data" type="button" class="button ghost">${t("action.cleanNow")}</button></div></section>
      </div>
      <footer><span>v${await getVersion()}</span><div><button type="button" class="button ghost modal-close">${t("action.cancel")}</button><button class="button primary">${t("action.save")}</button></div></footer>
    </form>`;
  byId<HTMLSelectElement>("retention-days").value = settings.retentionDays?.toString() ?? "unlimited";
  byId<HTMLSelectElement>("language").value = settings.language;
  dialog.querySelectorAll(".modal-close").forEach((button) =>
    button.addEventListener("click", () => dialog.close()),
  );
  byId("open-data").addEventListener("click", () => void openPath(storage.dataDirectory));
  byId("backup-data").addEventListener("click", async () => {
    try {
      const path = await api.backup();
      showToast(t("toast.backupCreated", { path }), "success");
    } catch (error) {
      showToast(formatError(error), "error");
    }
  });
  byId("cleanup-data").addEventListener("click", async () => {
    try {
      const deleted = await api.cleanup();
      showToast(t("toast.removedSamples", { count: deleted }), "success");
    } catch (error) {
      showToast(formatError(error), "error");
    }
  });
  byId<HTMLFormElement>("settings-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
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
      if (language === resolveLanguage(settings.language)) {
        showToast(t("toast.settingsSaved"), "success");
      }
    } catch (error) {
      showToast(formatError(error), "error");
    }
  });
  dialog.showModal();
}

async function initPopup(): Promise<void> {
  root.className = "popup-shell";
  root.innerHTML = `
    <section class="popup-card">
      <header><div class="brand-mark small"><img src="${logoUrl}" alt="" /></div><div><strong>LNPM</strong><span id="popup-overall"></span></div><button id="popup-close" aria-label="${t("action.close")}">×</button></header>
      <div id="popup-alert" class="popup-alert hidden"></div>
      <div id="popup-targets" class="popup-targets"></div>
      <div id="popup-chart" class="popup-chart"></div>
      <footer><button id="popup-pause" class="button ghost"></button><button id="popup-details" class="button primary">${t("action.details")} ↗</button></footer>
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
    ? t("action.resume")
    : t("action.pause");
  byId("popup-targets").innerHTML = dashboard.targets.length
    ? dashboard.targets
        .map(
          (item) => `<div class="popup-target-row"><span class="status-dot state-${item.state}"></span><strong>${escapeHtml(item.target.name)}</strong><small>${stateLabel(item.state, language)}</small><span>${formatLatency(item.latestSample?.latencyMs)}</span></div>`,
        )
        .join("")
    : `<div class="popup-empty">${t("empty.popup")}</div>`;
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

async function showAvailableUpdate(info: UpdateInfo): Promise<void> {
  updateUiState = reduceUpdateUiState(updateUiState, { type: "available", payload: info });
  if (isPopup) return;
  currentAppVersion ??= await getVersion();
  renderUpdateDialog();
  const dialog = byId<HTMLDialogElement>("update-dialog");
  if (!dialog.open) dialog.showModal();
}

function renderUpdateDialog(): void {
  if (isPopup || !updateUiState.info) return;
  const dialog = document.getElementById("update-dialog") as HTMLDialogElement | null;
  if (!dialog) return;
  const info = updateUiState.info;
  const busy = isUpdateBusy();
  const failure = updateUiState.phase === "failed" ? updateUiState.error : null;
  const statusKey =
    updateUiState.phase === "downloading"
      ? "update.downloading"
      : updateUiState.phase === "verifying"
        ? "update.verifying"
        : updateUiState.phase === "installing"
          ? "update.installing"
          : "update.restarting";
  const progress = updateUiState.percent;
  const statusContent = busy
    ? `<div class="update-status" aria-live="polite">
        <strong>${t(statusKey)}</strong>
        <progress max="100" ${progress == null ? "" : `value="${progress}"`}></progress>
        ${
          updateUiState.phase === "downloading" && progress != null
            ? `<small>${t("update.downloadProgress", { percent: new Intl.NumberFormat(language, { maximumFractionDigits: 1 }).format(progress) })}</small>`
            : ""
        }
      </div>`
    : failure
      ? `<div class="update-error" role="alert">${escapeHtml(formatError(failure))}</div>`
      : info.notes
        ? `<section class="update-notes"><strong>${t("update.notes")}</strong><p>${escapeHtml(info.notes)}</p></section>`
        : "";

  dialog.innerHTML = `
    <div class="modal-content update-content">
      <header>
        <div><span class="eyebrow">LNPM</span><h3>${t("update.title")}</h3></div>
        ${busy ? "" : `<button id="update-close" type="button" class="modal-close" aria-label="${t("action.close")}">×</button>`}
      </header>
      <p class="update-message">${t("update.message", { version: formatVersionLabel(info.version) })}</p>
      <dl class="update-versions">
        <div><dt>${t("update.currentVersion")}</dt><dd>${formatVersionLabel(currentAppVersion ?? "0.2.0")}</dd></div>
        <div><dt>${t("update.newVersion")}</dt><dd>${formatVersionLabel(info.version)}</dd></div>
      </dl>
      ${statusContent}
      <footer>
        ${
          busy
            ? ""
            : `<button id="update-skip" type="button" class="button ghost danger-text">${t("update.skipVersion")}</button>
               <div><button id="update-later" type="button" class="button ghost">${t("update.later")}</button><button id="update-install" type="button" class="button primary">${failure ? t("update.retry") : t("update.update")}</button></div>`
        }
      </footer>
    </div>`;

  if (busy) return;
  dialog.querySelector("#update-close")?.addEventListener("click", () => void deferCurrentUpdate());
  dialog.querySelector("#update-later")?.addEventListener("click", () => void deferCurrentUpdate());
  dialog.querySelector("#update-skip")?.addEventListener("click", () => void skipCurrentUpdate());
  dialog.querySelector("#update-install")?.addEventListener("click", () => void installCurrentUpdate());
}

async function deferCurrentUpdate(): Promise<void> {
  const info = updateUiState.info;
  if (!info || isUpdateBusy()) return;
  try {
    settings = await api.deferUpdate(info.version);
    byId<HTMLDialogElement>("update-dialog").close();
    updateUiState = reduceUpdateUiState(updateUiState, { type: "dismissed" });
  } catch (error) {
    showToast(formatError(error), "error");
  }
}

async function skipCurrentUpdate(): Promise<void> {
  const info = updateUiState.info;
  if (!info || isUpdateBusy()) return;
  try {
    settings = await api.skipUpdate(info.version);
    byId<HTMLDialogElement>("update-dialog").close();
    updateUiState = reduceUpdateUiState(updateUiState, { type: "dismissed" });
  } catch (error) {
    showToast(formatError(error), "error");
  }
}

async function installCurrentUpdate(): Promise<void> {
  const info = updateUiState.info;
  if (!info || isUpdateBusy()) return;
  updateUiState = reduceUpdateUiState(updateUiState, {
    type: "progress",
    payload: { version: info.version, status: "downloading", percent: 0 },
  });
  renderUpdateDialog();
  try {
    await api.installUpdate();
  } catch (error) {
    const payload = normalizeError(error);
    updateUiState = reduceUpdateUiState(updateUiState, {
      type: "failed",
      payload: { version: info.version, ...payload },
    });
    renderUpdateDialog();
  }
}

function isUpdateBusy(): boolean {
  return ["downloading", "verifying", "installing", "restarting"].includes(
    updateUiState.phase,
  );
}

function formatVersionLabel(version: string): string {
  return version.startsWith("v") ? version : `v${version}`;
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

const viewStateKey = "lnpm-view-state";

function applyDocumentLanguage(): void {
  document.documentElement.lang = language;
  document.title = isPopup ? t("app.popupTitle") : "LNPM";
}

function preserveViewState(): void {
  if (isPopup) return;
  sessionStorage.setItem(
    viewStateKey,
    JSON.stringify({ selectedTargetId, currentRange, followLive }),
  );
}

function restoreViewState(): void {
  if (isPopup) return;
  const stored = sessionStorage.getItem(viewStateKey);
  sessionStorage.removeItem(viewStateKey);
  if (!stored) return;
  try {
    const parsed = JSON.parse(stored) as {
      selectedTargetId?: unknown;
      currentRange?: { fromMs?: unknown; toMs?: unknown };
      followLive?: unknown;
    };
    selectedTargetId =
      typeof parsed.selectedTargetId === "string" ? parsed.selectedTargetId : selectedTargetId;
    if (
      typeof parsed.currentRange?.fromMs === "number" &&
      typeof parsed.currentRange.toMs === "number" &&
      parsed.currentRange.toMs > parsed.currentRange.fromMs
    ) {
      currentRange = {
        fromMs: parsed.currentRange.fromMs,
        toMs: parsed.currentRange.toMs,
      };
    }
    if (typeof parsed.followLive === "boolean") followLive = parsed.followLive;
    byId("follow-live").classList.toggle("active", followLive);
    if (!followLive) {
      root.querySelectorAll("[data-range]").forEach((button) => button.classList.remove("active"));
    }
  } catch (error) {
    console.debug("Unable to restore view state", error);
  }
}
