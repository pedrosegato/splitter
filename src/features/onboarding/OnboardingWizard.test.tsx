import { render, fireEvent, within, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { OnboardingWizard } from "./OnboardingWizard";

const mockComplete = vi.fn();
let mockOnboarded = false;

vi.mock("./useOnboarding", () => ({
  useOnboarding: (selector: (s: { onboarded: boolean; complete: () => void }) => unknown) =>
    selector({ onboarded: mockOnboarded, complete: mockComplete }),
}));

vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    data: { microphone: "not_applicable", screen: "not_applicable" },
  }),
  useRequestPermission: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
}));

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("OnboardingWizard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockOnboarded = false;
  });

  it("renders wizard when onboarded is false", () => {
    render(<OnboardingWizard />, { wrapper: makeWrapper() });
    expect(document.body.querySelector('[data-slot="dialog-content"]')).toBeTruthy();
    expect(within(document.body).getByText("Bem-vindo ao Splitter")).toBeTruthy();
  });

  it("does not render wizard when onboarded is true", () => {
    mockOnboarded = true;
    render(<OnboardingWizard />, { wrapper: makeWrapper() });
    expect(document.body.querySelector('[data-slot="dialog-content"]')).toBeNull();
  });

  it("advances through all steps and calls complete on Concluir", async () => {
    render(<OnboardingWizard />, { wrapper: makeWrapper() });

    const body = document.body;

    expect(within(body).getByText("Bem-vindo ao Splitter")).toBeTruthy();

    fireEvent.click(within(body).getByRole("button", { name: "Próximo" }));
    expect(within(body).getByText("Permissões")).toBeTruthy();

    fireEvent.click(within(body).getByRole("button", { name: "Próximo" }));
    expect(within(body).getByText("Rede")).toBeTruthy();

    fireEvent.click(within(body).getByRole("button", { name: "Próximo" }));
    expect(within(body).getByText("Pronto")).toBeTruthy();

    await waitFor(() =>
      expect(within(body).getByRole("button", { name: "Concluir" })).toBeTruthy(),
    );
    fireEvent.click(within(body).getByRole("button", { name: "Concluir" }));
    expect(mockComplete).toHaveBeenCalledTimes(1);
  });

  it("shows the current step content after navigating forward and back", async () => {
    render(<OnboardingWizard />, { wrapper: makeWrapper() });

    const body = document.body;

    fireEvent.click(within(body).getByRole("button", { name: "Próximo" }));
    expect(within(body).getByText("Permissões")).toBeTruthy();

    fireEvent.click(within(body).getByRole("button", { name: "Voltar" }));
    await waitFor(() =>
      expect(within(body).getByText("Bem-vindo ao Splitter")).toBeTruthy(),
    );
  });
});
