import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import App from "./App";

describe("foundation health screen", () => {
  test("states the current boundary without exposing product controls", () => {
    const { container } = render(<App />);

    expect(screen.getByRole("main")).toHaveAccessibleName("Flit foundation");
    expect(screen.getByRole("heading", { level: 1, name: "Flit foundation" })).toBeVisible();
    expect(screen.getByRole("status")).toHaveTextContent("Foundation ready");
    expect(screen.getByText(/Provider monitoring has not started/)).toBeVisible();
    expect(screen.getByText("No agent controls yet")).toBeVisible();
    expect(container.querySelectorAll("a, button, input, select, textarea")).toHaveLength(0);
  });
});
