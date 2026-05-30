import type { StreamStat } from "@/bindings";

export type AggregateResult = {
  avgRtt: number;
  avgLoss: number;
  totalKbps: number;
};

export function aggregate(stats: StreamStat[]): AggregateResult {
  if (stats.length === 0) {
    return { avgRtt: 0, avgLoss: 0, totalKbps: 0 };
  }

  const sum = stats.reduce(
    (acc, s) => ({
      rtt: acc.rtt + s.rtt_ms,
      loss: acc.loss + s.loss_pct,
      kbps: acc.kbps + s.kbps_sent + s.kbps_received,
    }),
    { rtt: 0, loss: 0, kbps: 0 },
  );

  return {
    avgRtt: sum.rtt / stats.length,
    avgLoss: sum.loss / stats.length,
    totalKbps: sum.kbps,
  };
}
