import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "./tabs";

describe("Tabs", () => {
  it("renders triggers and switches active content", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
          <TabsTrigger value="b">B</TabsTrigger>
        </TabsList>
        <TabsContent value="a">Painel A</TabsContent>
        <TabsContent value="b">Painel B</TabsContent>
      </Tabs>,
    );
    expect(screen.getByText("Painel A")).toBeInTheDocument();
  });

  it("marks the active trigger with data-state", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
          <TabsTrigger value="b">B</TabsTrigger>
        </TabsList>
        <TabsContent value="a">Painel A</TabsContent>
        <TabsContent value="b">Painel B</TabsContent>
      </Tabs>,
    );
    expect(screen.getByRole("tab", { name: "A" })).toHaveAttribute("data-state", "active");
    expect(screen.getByRole("tab", { name: "B" })).toHaveAttribute("data-state", "inactive");
  });
});
