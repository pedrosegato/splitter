export function deviceLabel(raw: string): string {
  return raw.replace(/^[A-Za-z]+:\d+:/, "");
}
