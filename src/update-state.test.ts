import { describe, expect, it } from "vitest";

import { initialUpdateUiState, reduceUpdateUiState } from "./update-state";

describe("update UI events", () => {
  it("tracks an available update and download progress", () => {
    const available = reduceUpdateUiState(initialUpdateUiState, {
      type: "available",
      payload: { version: "0.3.0", notes: "Signed update" },
    });
    const downloading = reduceUpdateUiState(available, {
      type: "progress",
      payload: { version: "0.3.0", status: "downloading", percent: 42.5 },
    });

    expect(downloading.info?.version).toBe("0.3.0");
    expect(downloading.phase).toBe("downloading");
    expect(downloading.percent).toBe(42.5);
  });

  it("ignores stale-version progress and preserves retryable failures", () => {
    const available = reduceUpdateUiState(initialUpdateUiState, {
      type: "available",
      payload: { version: "0.4.0" },
    });
    expect(
      reduceUpdateUiState(available, {
        type: "progress",
        payload: { version: "0.3.0", status: "installing" },
      }),
    ).toBe(available);

    const failed = reduceUpdateUiState(available, {
      type: "failed",
      payload: { version: "0.4.0", code: "updateSignature", detail: "bad signature" },
    });
    expect(failed.phase).toBe("failed");
    expect(failed.error?.code).toBe("updateSignature");
  });
});
