import { useMemo } from "react";
import { motion } from "motion/react";
import type { StreamStat, StreamSnapshot } from "@/bindings";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import type { StreamHistory } from "@/stores/ui";
import { useSnapshot } from "@/hooks/useSnapshot";
import { useActiveSession } from "@/hooks/useActiveSession";
import { streamColor } from "@/features/routing/useWireGeometry";
import { deviceLabel } from "@/lib/deviceName";
import { Sparkline } from "@/components/Sparkline";
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { variants } from "@/lib/motion";
import { aggregate } from "./aggregate";

function MetricCard({
  value,
  unit,
  label,
  accent,
}: {
  value: string;
  unit?: string;
  label: string;
  accent?: boolean;
}) {
  return (
    <motion.div
      variants={variants.listItem}
      className="rounded-xl border border-line bg-surface px-4 py-3.5"
    >
      <div className="flex items-baseline gap-1">
        <span
          className={cn(
            "text-2xl tabular-nums leading-none",
            accent ? "text-gold" : "text-ink",
          )}
        >
          {value}
        </span>
        {unit && <span className="text-xs text-ink-3">{unit}</span>}
      </div>
      <div className="text-[11px] text-ink-3 mt-2">{label}</div>
    </motion.div>
  );
}

function Metric({
  value,
  unit,
  values,
  color,
  testId,
}: {
  value: string;
  unit: string;
  values: number[];
  color: string;
  testId: string;
}) {
  return (
    <div className="flex flex-col items-end gap-1">
      <div className="text-xs tabular-nums">
        <span className="text-ink">{value}</span>
        <span className="text-ink-3 ml-0.5">{unit}</span>
      </div>
      <span data-testid={testId} className="opacity-70">
        <Sparkline values={values} width={54} height={16} color={color} />
      </span>
    </div>
  );
}

function StreamRow({
  stat,
  stream,
  history,
}: {
  stat: StreamStat;
  stream: StreamSnapshot | undefined;
  history: StreamHistory | undefined;
}) {
  const color = streamColor(stat.stream_id);
  const source = stream ? deviceLabel(stream.source_device) : `stream ${stat.stream_id}`;
  const sink = stream ? deviceLabel(stream.sink_device) : undefined;

  return (
    <motion.div
      variants={variants.listItem}
      className="flex items-center gap-3 rounded-lg px-3 py-2.5 transition-colors hover:bg-surface"
    >
      <span
        className="size-2.5 flex-none rounded-full ring-2 ring-inset ring-black/10"
        style={{ background: color }}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 truncate text-xs">
          <span className="min-w-0 truncate text-ink-2">{source}</span>
          {sink && (
            <>
              <span className="flex-none text-gold">→</span>
              <span className="min-w-0 truncate text-ink">{sink}</span>
            </>
          )}
        </div>
      </div>
      <div className="flex flex-none items-start gap-5">
        <Metric
          value={String(stat.rtt_ms)}
          unit="ms"
          values={history?.rtt ?? []}
          color={color}
          testId={`sparkline-rtt-${stat.stream_id}`}
        />
        <Metric
          value={stat.loss_pct.toFixed(1)}
          unit="%"
          values={history?.loss ?? []}
          color="var(--color-ink-3)"
          testId={`sparkline-loss-${stat.stream_id}`}
        />
        <Metric
          value={String(stat.kbps_sent + stat.kbps_received)}
          unit="kbps"
          values={history?.kbps ?? []}
          color={color}
          testId={`sparkline-kbps-${stat.stream_id}`}
        />
      </div>
    </motion.div>
  );
}

export function StatsView() {
  const stats = useUiStore((s) => s.stats);
  const statsHistory = useUiStore((s) => s.statsHistory);
  const { data: sessions } = useSnapshot();
  const { session: activeSession } = useActiveSession();
  const activeStreamCount = activeSession?.streams.length ?? 0;

  const allStreams = useMemo<StreamSnapshot[]>(
    () => sessions?.flatMap((s) => s.streams) ?? [],
    [sessions],
  );

  const streamById = useMemo(
    () => new Map(allStreams.map((s) => [s.id, s])),
    [allStreams],
  );

  const { avgRtt, avgLoss, totalKbps } = aggregate(stats);

  return (
    <div className="overflow-auto p-6">
      <motion.div
        variants={variants.listStagger}
        initial="hidden"
        animate="show"
        className="mb-8 grid gap-3"
        style={{ gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))" }}
      >
        <MetricCard value={String(activeStreamCount)} label="streams ativos" accent />
        <MetricCard value={String(Math.round(avgRtt))} unit="ms" label="latência média" />
        <MetricCard value={avgLoss.toFixed(1)} unit="%" label="perda média" />
        <MetricCard value={String(Math.round(totalKbps))} unit="kbps" label="banda total" />
      </motion.div>

      <p className="mb-2 px-3 text-[11px] text-ink-3">Por stream</p>

      {stats.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyTitle>sem streams ativos</EmptyTitle>
          </EmptyHeader>
        </Empty>
      ) : (
        <motion.div
          variants={variants.listStagger}
          initial="hidden"
          animate="show"
          className="rounded-xl border border-line bg-board/40 p-1"
        >
          {stats.map((stat) => (
            <StreamRow
              key={`${stat.session_id}-${stat.stream_id}`}
              stat={stat}
              stream={streamById.get(stat.stream_id)}
              history={statsHistory[stat.stream_id]}
            />
          ))}
        </motion.div>
      )}
    </div>
  );
}
