import { describe, it, expect, beforeEach } from "vitest";
import { useThemeStore, applyTheme } from "./theme";

beforeEach(() => {
  useThemeStore.setState({ theme: "dark" });
  document.documentElement.className = "";
});

describe("useThemeStore", () => {
  it("starts with dark theme", () => {
    expect(useThemeStore.getState().theme).toBe("dark");
  });

  it("setTheme switches to light", () => {
    useThemeStore.getState().setTheme("light");
    expect(useThemeStore.getState().theme).toBe("light");
  });

  it("setTheme switches back to dark", () => {
    useThemeStore.getState().setTheme("light");
    useThemeStore.getState().setTheme("dark");
    expect(useThemeStore.getState().theme).toBe("dark");
  });

  it("toggle flips dark to light", () => {
    useThemeStore.getState().toggle();
    expect(useThemeStore.getState().theme).toBe("light");
  });

  it("toggle flips light to dark", () => {
    useThemeStore.setState({ theme: "light" });
    useThemeStore.getState().toggle();
    expect(useThemeStore.getState().theme).toBe("dark");
  });

  it("toggle alternates on repeated calls", () => {
    useThemeStore.getState().toggle();
    expect(useThemeStore.getState().theme).toBe("light");
    useThemeStore.getState().toggle();
    expect(useThemeStore.getState().theme).toBe("dark");
  });
});

describe("applyTheme", () => {
  it("adds dark class when theme is dark", () => {
    applyTheme("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("removes dark class when theme is light", () => {
    document.documentElement.classList.add("dark");
    applyTheme("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("is idempotent for dark", () => {
    applyTheme("dark");
    applyTheme("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("is idempotent for light", () => {
    applyTheme("light");
    applyTheme("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });
});
