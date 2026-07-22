import { invoke } from "@tauri-apps/api/core";

import {
  PROTOCOL_VERSION,
  type SystemHealthRequest,
  type SystemHealthResponse,
} from "./generated/protocol";

export async function loadSystemHealth(): Promise<SystemHealthResponse> {
  const request: SystemHealthRequest = {
    client_protocol_version: PROTOCOL_VERSION,
  };
  const response = await invoke<SystemHealthResponse>("system_health", { request });

  if (response.protocol_version !== PROTOCOL_VERSION) {
    throw new Error("The local Core returned an incompatible protocol version.");
  }

  return response;
}
