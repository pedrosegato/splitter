import { render, fireEvent, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { SettingsDialog } from "./SettingsDialog";
import { useThemeStore, applyTheme } from "@/stores/theme";
import type { Settings } from "@/bindings";

const mockSet = vi.fn();
const mockSetAutostart = vi.fn();

const defaultSettings: Settings = {
  auto_accept_trusted: false,
  auto_start_with_system: false,
  default_bitrate: 128000,
  fec_mode: "auto",
  fec_on_threshold_pct: 5,
  fec_off_threshold_pct: 2,
  fec_hysteresis_secs: 3,
  jitter_mode: "auto",
  jitter_max_depth_ms: 80,
  log_level: "info",
  metrics_enabled: false,
  metrics_port: 9090,
  signaling_port: 7373,
};

vi.mock("./useSettingsForm", () => ({
  useSettingsForm: () => ({
    settings: defaultSettings,
    isLoading: false,
    isSaved: false,
    set: mockSet,
    setAutostart: mockSetAutostart,
  }),
}));

vi.mock("@/stores/theme", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/stores/theme")>();
  return {
    ...actual,
    applyTheme: vi.fn(),
  };
});

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("SettingsDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useThemeStore.setState({ theme: "dark" });
    document.documentElement.className = "";
  });

  it("renders dialog with settings sections", () => {
    render(<SettingsDialog open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const body = document.body;
    expect(within(body).getByText("Configurações")).toBeTruthy();
    expect(within(body).getByText("Conexão")).toBeTruthy();
    expect(within(body).getByText("Áudio")).toBeTruthy();
    expect(within(body).getByText("Sistema")).toBeTruthy();
    expect(within(body).getByText("Aparência")).toBeTruthy();
  });

  it("toggling auto_accept_trusted switch calls set('auto_accept_trusted', true)", () => {
    render(<SettingsDialog open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const switchEl = document.body.querySelector(
      '[id="auto-accept-trusted"]',
    ) as HTMLButtonElement;
    expect(switchEl).toBeTruthy();

    fireEvent.click(switchEl);

    expect(mockSet).toHaveBeenCalledWith("auto_accept_trusted", true);
  });

  it("theme Escuro button sets dark theme and calls applyTheme", () => {
    useThemeStore.setState({ theme: "light" });

    render(<SettingsDialog open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const btn = within(document.body).getByRole("button", { name: "Escuro" });
    fireEvent.click(btn);

    expect(useThemeStore.getState().theme).toBe("dark");
    expect(applyTheme).toHaveBeenCalledWith("dark");
  });

  it("theme Claro button sets light theme and calls applyTheme", () => {
    render(<SettingsDialog open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const btn = within(document.body).getByRole("button", { name: "Claro" });
    fireEvent.click(btn);

    expect(useThemeStore.getState().theme).toBe("light");
    expect(applyTheme).toHaveBeenCalledWith("light");
  });

  it("fechar button calls onOpenChange(false)", () => {
    const onOpenChange = vi.fn();
    render(<SettingsDialog open onOpenChange={onOpenChange} />, {
      wrapper: makeWrapper(),
    });

    const btn = within(document.body).getByRole("button", { name: "fechar" });
    fireEvent.click(btn);

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("auto_start_with_system switch calls setAutostart with true when toggled", () => {
    render(<SettingsDialog open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const switchEl = document.body.querySelector(
      '[id="auto-start-system"]',
    ) as HTMLButtonElement;
    expect(switchEl).toBeTruthy();

    fireEvent.click(switchEl);

    expect(mockSetAutostart).toHaveBeenCalledWith(true);
    expect(mockSet).not.toHaveBeenCalledWith("auto_start_with_system", expect.anything());
  });

  it("metrics_enabled switch calls set with true when toggled", () => {
    render(<SettingsDialog open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const switchEl = document.body.querySelector(
      '[id="metrics-enabled"]',
    ) as HTMLButtonElement;
    expect(switchEl).toBeTruthy();

    fireEvent.click(switchEl);

    expect(mockSet).toHaveBeenCalledWith("metrics_enabled", true);
  });
});
