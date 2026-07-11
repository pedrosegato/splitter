import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { Card, CardHeader, CardTitle, CardContent } from "./card";

describe("Card", () => {
  it("composes header and content", () => {
    render(
      <Card>
        <CardHeader><CardTitle>Latência</CardTitle></CardHeader>
        <CardContent>12 ms</CardContent>
      </Card>,
    );
    expect(screen.getByText("Latência")).toBeInTheDocument();
    expect(screen.getByText("12 ms")).toBeInTheDocument();
  });
});
