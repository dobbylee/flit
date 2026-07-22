import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { loadSystemHealth } from "./systemHealth";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

describe("system health bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  test("sends the generated client protocol version", async () => {
    invokeMock.mockResolvedValue({
      protocol_version: "1.0",
      core: "ready",
      storage: "not_configured",
      providers: "not_configured",
    });

    await expect(loadSystemHealth()).resolves.toMatchObject({ core: "ready" });
    expect(invokeMock).toHaveBeenCalledWith("system_health", {
      request: { client_protocol_version: "1.0" },
    });
  });

  test("rejects an incompatible Core response", async () => {
    invokeMock.mockResolvedValue({
      protocol_version: "2.0",
      core: "ready",
      storage: "not_configured",
      providers: "not_configured",
    });

    await expect(loadSystemHealth()).rejects.toThrow("incompatible protocol version");
  });
});
