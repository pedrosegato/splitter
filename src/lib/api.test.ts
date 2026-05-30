import { describe, it, expect } from "vitest";
import { unwrap } from "./api";

describe("unwrap", () => {
  it("returns data when status is ok", async () => {
    const result = await unwrap(Promise.resolve({ status: "ok", data: 42 } as const));
    expect(result).toBe(42);
  });

  it("throws with the error message when status is error", async () => {
    await expect(
      unwrap(Promise.resolve({ status: "error", error: "something went wrong" } as const))
    ).rejects.toThrow("something went wrong");
  });
});
