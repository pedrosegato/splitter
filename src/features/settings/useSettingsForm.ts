import { useSettings, useSetSetting } from "@/hooks/useSettings";
import type { JitterMode } from "@/bindings";

function toBackendString(key: string, value: string | number | boolean): string {
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return String(value);
  return value;
}

function jitterModeToBackend(mode: JitterMode): string {
  if (mode === "auto" || mode === "min") return mode;
  return `fixed:${mode.fixed}`;
}

export function useSettingsForm() {
  const query = useSettings();
  const mutation = useSetSetting();

  function set(key: string, value: string | number | boolean) {
    let stringValue: string;
    if (key === "jitter_mode" && typeof value === "object" && value !== null) {
      stringValue = jitterModeToBackend(value as JitterMode);
    } else if (key === "jitter_mode" && typeof value === "string") {
      stringValue = value;
    } else {
      stringValue = toBackendString(key, value);
    }
    mutation.mutate({ key, value: stringValue });
  }

  return {
    settings: query.data,
    isLoading: query.isLoading,
    isSaved: mutation.isSuccess,
    set,
  };
}
