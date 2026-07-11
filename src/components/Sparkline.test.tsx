import { render } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { Sparkline } from "./Sparkline";

describe("Sparkline", () => {
  it("renders an svg element", () => {
    const { container } = render(<Sparkline values={[1, 2, 3]} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
  });

  it("renders a polyline with one point per value", () => {
    const values = [10, 20, 30, 40, 50];
    const { container } = render(<Sparkline values={values} />);
    const polyline = container.querySelector("polyline");
    expect(polyline).not.toBeNull();
    const points = polyline!.getAttribute("points")!.trim().split(" ");
    expect(points).toHaveLength(values.length);
  });

  it("renders 60 points for a 60-value array", () => {
    const values = Array.from({ length: 60 }, (_, i) => i + 1);
    const { container } = render(<Sparkline values={values} />);
    const polyline = container.querySelector("polyline");
    const points = polyline!.getAttribute("points")!.trim().split(" ");
    expect(points).toHaveLength(60);
  });

  it("renders without crashing when values is empty", () => {
    const { container } = render(<Sparkline values={[]} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    const polyline = container.querySelector("polyline");
    expect(polyline).toBeNull();
  });

  it("uses the provided width and height", () => {
    const { container } = render(<Sparkline values={[1, 2]} width={120} height={32} />);
    const svg = container.querySelector("svg");
    expect(svg!.getAttribute("width")).toBe("120");
    expect(svg!.getAttribute("height")).toBe("32");
  });

  it("applies custom color to polyline stroke", () => {
    const { container } = render(<Sparkline values={[1, 2, 3]} color="#ff0000" />);
    const polyline = container.querySelector("polyline");
    expect(polyline!.getAttribute("stroke")).toBe("#ff0000");
  });

  it("uses provided max to scale y-axis", () => {
    const { container } = render(
      <Sparkline values={[50]} width={80} height={24} max={100} />,
    );
    const polyline = container.querySelector("polyline");
    const pointStr = polyline!.getAttribute("points")!;
    const [, y] = pointStr.trim().split(",").map(Number);
    expect(y).toBeCloseTo(12, 1);
  });

  it("renders a single-value array without crashing", () => {
    const { container } = render(<Sparkline values={[42]} />);
    const polyline = container.querySelector("polyline");
    expect(polyline).not.toBeNull();
    const points = polyline!.getAttribute("points")!.trim().split(" ");
    expect(points).toHaveLength(1);
  });

  it("renders a polyline when values exist", () => {
    const { container } = render(<Sparkline values={[1, 2, 3]} />);
    expect(container.querySelector("polyline")).toBeInTheDocument();
  });

  it("renders nothing drawable for empty values", () => {
    const { container } = render(<Sparkline values={[]} />);
    expect(container.querySelector("polyline")).not.toBeInTheDocument();
  });
});
