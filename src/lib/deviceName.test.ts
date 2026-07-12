import { describe, it, expect } from "vitest";
import { deviceLabel } from "./deviceName";

describe("deviceLabel", () => {
  it("strips the Kind:index: prefix from a system audio route id", () => {
    expect(
      deviceLabel("SystemAudio:0:3 - Odyssey G40B (2- AMD High Definition Audio Device)"),
    ).toBe("3 - Odyssey G40B (2- AMD High Definition Audio Device)");
  });

  it("strips the Output:index: prefix", () => {
    expect(deviceLabel("Output:0:MCHOSE V9 PRO")).toBe("MCHOSE V9 PRO");
  });

  it("leaves a plain device name untouched", () => {
    expect(deviceLabel("Microfone (MacBook Air)")).toBe("Microfone (MacBook Air)");
  });

  it("only strips a leading prefix, not colons inside the name", () => {
    expect(deviceLabel("Input:1:Zoom H6: main")).toBe("Zoom H6: main");
  });
});
