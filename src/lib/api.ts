import { commands, events } from "@/bindings";

export { commands, events };

type Result<T, E> = { status: "ok"; data: T } | { status: "error"; error: E };

export async function unwrap<T>(p: Promise<Result<T, string>>): Promise<T> {
  const r = await p;
  if (r.status === "ok") return r.data;
  throw new Error(r.error);
}
