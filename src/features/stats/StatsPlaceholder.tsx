import { useUiStore } from "@/stores/ui";

export function StatsPlaceholder() {
  const stats = useUiStore((s) => s.stats);

  return (
    <div className="p-4">
      <span className="text-ink-2 text-xs">
        {stats.length} stream{stats.length !== 1 ? "s" : ""} com stats
      </span>
    </div>
  );
}
