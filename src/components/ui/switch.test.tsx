import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Switch } from "./switch";

describe("Switch", () => {
  it("reflects checked state", () => {
    render(<Switch checked aria-label="mute" onCheckedChange={() => {}} />);
    expect(screen.getByRole("switch")).toHaveAttribute("data-state", "checked");
  });
});
