import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import App from "./App";
import { loadSystemHealth } from "./systemHealth";

vi.mock("./systemHealth", () => ({
  loadSystemHealth: vi.fn(),
}));

const loadSystemHealthMock = vi.mocked(loadSystemHealth);

describe("foundation health screen", () => {
  beforeEach(() => {
    loadSystemHealthMock.mockReset();
  });

  test("shows a truthful loading boundary before the Core responds", () => {
    loadSystemHealthMock.mockReturnValue(new Promise(() => undefined));

    render(<App />);

    expect(screen.getByRole("status")).toHaveTextContent("Checking foundation");
    expect(screen.getByText(/Verifying the local Core contract/)).toBeVisible();
  });

  test("shows the verified foundation boundary without product controls", async () => {
    loadSystemHealthMock.mockResolvedValue({
      protocol_version: "1.0",
      core: "ready",
      storage: "not_configured",
      providers: "not_configured",
    });

    const { container } = render(<App />);

    expect(screen.getByRole("main")).toHaveAccessibleName("Flit foundation");
    expect(screen.getByRole("heading", { level: 1, name: "Flit foundation" })).toBeVisible();
    expect(await screen.findByText("Core contract verified")).toHaveAttribute("role", "status");
    expect(screen.getByText(/Storage and provider monitoring have not started/)).toBeVisible();
    expect(screen.getByText("No agent controls yet")).toBeVisible();
    expect(container.querySelectorAll("a, button, input, select, textarea")).toHaveLength(0);
  });

  test("fails closed when the Core contract cannot be verified", async () => {
    loadSystemHealthMock.mockRejectedValue(new Error("protocol mismatch"));

    render(<App />);

    expect(await screen.findByText("Foundation unavailable")).toHaveAttribute("role", "status");
    expect(screen.getByText(/No agent controls are available/)).toBeVisible();
  });
});
