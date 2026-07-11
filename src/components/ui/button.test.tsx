import "@testing-library/jest-dom";
import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { Button } from "./button";

describe("Button", () => {
  it("renders children and forwards variant/size data attributes", () => {
    render(<Button variant="secondary" size="sm">Salvar</Button>);
    const el = screen.getByRole("button", { name: "Salvar" });
    expect(el).toHaveAttribute("data-variant", "secondary");
    expect(el).toHaveAttribute("data-size", "sm");
  });

  it("still supports asChild without motion wrapping", () => {
    render(<Button asChild><a href="/x">Link</a></Button>);
    expect(screen.getByRole("link", { name: "Link" })).toBeInTheDocument();
  });
});
