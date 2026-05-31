import { useMemo } from "react";
import type { StreamStat, StreamSnapshot } from "@/bindings";
import { useUiStore } from "@/stores/ui";
import type { StreamHistory } from "@/stores/ui";
import { useSnapshot } from "@/hooks/useSnapshot";
import { streamColor } from "@/features/routing/useWireGeometry";
import { Sparkline } from "@/components/Sparkline";
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
    <div className="bg-bg p-4">
      <div
        className={`font-mono text-2xl tabular-nums leading-none ${accent ? "text-gold" : "text-ink"}`}
      >
        {value}
        {unit && (
          <span className="text-sm text-ink-3 ml-0.5">{unit}</span>
        )}
      </div>
      <div className="text-[9px] text-ink-3 uppercase tracking-widest mt-1.5">
        {label}
      </div>
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
  const label = stream
    ? `${stream.source_device} → ${stream.sink_device}`
    : `stream ${stat.stream_id}`;

  const rttHistory = history?.rtt ?? [];
  const lossHistory = history?.loss ?? [];
  const kbpsHistory = history?.kbps ?? [];

  return (
    <div className="flex items-center gap-3 py-2.5 px-4 border-b border-line last:border-b-0">
      <span
        className="w-[9px] h-[30px] rounded-[2px] flex-none"
        style={{ background: color }}
      />
      <div className="flex-1 min-w-0">
        <div className="text-xs text-ink truncate">{label}</div>
      </div>
      <div className="flex items-center gap-4 text-xs tabular-nums text-ink-2 flex-none">
        <span className="flex items-center gap-1.5">
          <span className="text-ink">{stat.rtt_ms}</span>
          <span className="text-ink-3">ms</span>
          <span data-testid={`sparkline-rtt-${stat.stream_id}`}>
            <Sparkline values={rttHistory} width={60} height={20} color={color} />
          </span>
        </span>
        <span className="flex items-center gap-1.5">
          <span className="text-ink">{stat.loss_pct.toFixed(1)}</span>
          <span className="text-ink-3">%</span>
          <span data-testid={`sparkline-loss-${stat.stream_id}`}>
            <Sparkline values={lossHistory} width={60} height={20} color="var(--color-ink-3)" />
          </span>
        </span>
        <span className="flex items-center gap-1.5">
          <span className="text-ink">{stat.kbps_sent + stat.kbps_received}</span>
          <span className="text-ink-3">kbps</span>
          <span data-testid={`sparkline-kbps-${stat.stream_id}`}>
            <Sparkline values={kbpsHistory} width={60} height={20} color={color} />
          </span>
        </span>
      </div>
    </div>
  );
}

export function StatsView() {
  const stats = useUiStore((s) => s.stats);
  const statsHistory = useUiStore((s) => s.statsHistory);
  const { data: sessions } = useSnapshot();

  const activeSession = sessions?.find((s) => s.state === "active");
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
    <div className="p-6 overflow-auto">
      <p className="text-[9px] text-ink-3 uppercase tracking-widest mb-3">
        Sessão · agregado
      </p>

      <div
        className="grid gap-px bg-line border border-line mb-6"
        style={{ gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))" }}
      >
        <MetricCard
          value={String(activeStreamCount)}
          label="streams ativos"
          accent
        />
        <MetricCard
          value={String(Math.round(avgRtt))}
          unit="ms"
          label="latência média"
        />
        <MetricCard
          value={avgLoss.toFixed(1)}
          unit="%"
          label="perda média"
        />
        <MetricCard
          value={String(Math.round(totalKbps))}
          unit="kbps"
          label="banda total"
        />
        <MetricCard value="48" unit="kHz" label="sample rate" />
      </div>

      <p className="text-[9px] text-ink-3 uppercase tracking-widest mb-3">
        Por stream
      </p>

      <div className="border border-line">
        {stats.length === 0 ? (
          <div className="px-4 py-3 text-xs text-ink-3">
            sem streams ativos
          </div>
        ) : (
          stats.map((stat) => {
            const stream = streamById.get(stat.stream_id);
            return (
              <StreamRow
                key={`${stat.session_id}-${stat.stream_id}`}
                stat={stat}
                stream={stream}
                history={statsHistory[stat.stream_id]}
              />
            );
          })
        )}
      </div>
    </div>
  );
}
