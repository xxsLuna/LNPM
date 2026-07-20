import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  DashboardSnapshot,
  HistoryResponse,
  PingSample,
  StorageInfo,
  Target,
} from "./types";

export const api = {
  dashboard: () => invoke<DashboardSnapshot>("get_dashboard"),
  targets: () => invoke<Target[]>("list_targets"),
  createTarget: (name: string, host: string) =>
    invoke<Target>("create_target", { name, host }),
  saveTarget: (target: Target) => invoke<Target>("save_target", { target }),
  archiveTarget: (targetId: string) => invoke<void>("archive_target", { targetId }),
  pause: (paused: boolean) => invoke<void>("set_monitoring_paused", { paused }),
  testTarget: (target: Target) => invoke<PingSample>("test_target", { target }),
  history: (
    targetIds: string[],
    fromMs: number,
    toMs: number,
    maxPoints = 2_000,
  ) =>
    invoke<HistoryResponse>("get_history", {
      targetIds,
      fromMs,
      toMs,
      maxPoints,
    }),
  settings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) =>
    invoke<AppSettings>("save_settings", { settings }),
  storageInfo: () => invoke<StorageInfo>("get_storage_info"),
  cleanup: () => invoke<number>("run_retention_cleanup"),
  backup: () => invoke<string>("backup_database"),
  showMain: () => invoke<void>("show_main"),
  hidePopup: () => invoke<void>("hide_popup"),
  quit: () => invoke<void>("quit_app"),
};
