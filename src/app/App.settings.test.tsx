import { render, fireEvent, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { ReactNode } from "react";

vi.mock("@/lib/events", () => ({
  mountEventBridge: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@/hooks/useSnapshot", () => ({
  useSnapshot: vi.fn(() => ({ data: [] })),
}));

vi.mock("@/features/routing/RoutingBoard", () => ({
  RoutingBoard: () => <div data-testid="routing-board" />,
}));

vi.mock("@/features/stats/StatsView", () => ({
  StatsView: () => <div data-testid="stats-view" />,
}));

vi.mock("@/features/settings/SettingsDialog", () => ({
  SettingsDialog: ({ open }: { open: boolean; onOpenChange: (o: boolean) => void }) =>
    open ? <div data-testid="settings-dialog">Configurações</div> : null,
}));

import { App } from "./App";

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("App — settings gear button", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the gear button in the topbar", () => {
    const { getByRole } = render(<App />, { wrapper: makeWrapper() });
    expect(getByRole("button", { name: "Configurações" })).toBeDefined();
  });

  it("settings dialog is not visible on initial render", () => {
    render(<App />, { wrapper: makeWrapper() });
    expect(document.querySelector('[data-testid="settings-dialog"]')).toBeNull();
  });

  it("clicking the gear button opens the settings dialog", () => {
    const { getByRole } = render(<App />, { wrapper: makeWrapper() });
    const gearBtn = getByRole("button", { name: "Configurações" });
    fireEvent.click(gearBtn);
    expect(within(document.body).getByTestId("settings-dialog")).toBeDefined();
  });

  it("settings dialog content appears in document.body after gear click", () => {
    render(<App />, { wrapper: makeWrapper() });
    expect(document.body.querySelector('[data-testid="settings-dialog"]')).toBeNull();
    const gearBtn = within(document.body).getByRole("button", { name: "Configurações" });
    fireEvent.click(gearBtn);
    expect(document.body.querySelector('[data-testid="settings-dialog"]')).toBeTruthy();
  });
});
