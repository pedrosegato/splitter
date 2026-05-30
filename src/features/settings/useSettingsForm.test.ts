import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { ReactNode } from "react";

const mockMutate = vi.fn();

vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({
    data: {
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
    },
    isLoading: false,
  }),
  useSetSetting: () => ({
    mutate: mockMutate,
    isSuccess: false,
  }),
}));

import { useSettingsForm } from "./useSettingsForm";

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    QueryClientProvider({ client: queryClient, children })
  );
}

describe("useSettingsForm", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("set bool true → mutate with 'true'", () => {
    const { wrapper } = (() => {
      const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      return {
        wrapper: ({ children }: { children: ReactNode }) =>
          QueryClientProvider({ client: qc, children }),
      };
    })();

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("auto_accept_trusted", true);
    expect(mockMutate).toHaveBeenCalledWith({ key: "auto_accept_trusted", value: "true" });
  });

  it("set bool false → mutate with 'false'", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) =>
      QueryClientProvider({ client: qc, children });

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("metrics_enabled", false);
    expect(mockMutate).toHaveBeenCalledWith({ key: "metrics_enabled", value: "false" });
  });

  it("set number → mutate with string", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) =>
      QueryClientProvider({ client: qc, children });

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("default_bitrate", 96000);
    expect(mockMutate).toHaveBeenCalledWith({ key: "default_bitrate", value: "96000" });
  });

  it("set fec_mode string → mutate with 'always'", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) =>
      QueryClientProvider({ client: qc, children });

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("fec_mode", "always");
    expect(mockMutate).toHaveBeenCalledWith({ key: "fec_mode", value: "always" });
  });

  it("set jitter_mode object {fixed:40} → mutate with 'fixed:40'", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) =>
      QueryClientProvider({ client: qc, children });

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("jitter_mode", { fixed: 40 } as unknown as string);
    expect(mockMutate).toHaveBeenCalledWith({ key: "jitter_mode", value: "fixed:40" });
  });

  it("set jitter_mode 'auto' string → mutate with 'auto'", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) =>
      QueryClientProvider({ client: qc, children });

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("jitter_mode", "auto");
    expect(mockMutate).toHaveBeenCalledWith({ key: "jitter_mode", value: "auto" });
  });

  it("set jitter_mode 'fixed:60' string (pre-built) → mutate with 'fixed:60'", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) =>
      QueryClientProvider({ client: qc, children });

    const { result } = renderHook(() => useSettingsForm(), { wrapper });
    result.current.set("jitter_mode", "fixed:60");
    expect(mockMutate).toHaveBeenCalledWith({ key: "jitter_mode", value: "fixed:60" });
  });
});
