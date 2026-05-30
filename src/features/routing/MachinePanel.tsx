import { cn } from "@/lib/utils";
import { Port } from "./Port";

type Dev = { id: string; name: string };

type MachinePanelProps = {
  peerId: string;
  name: string;
  side: "left" | "right";
  isSelf?: boolean;
  connected: boolean;
  sinks: Dev[];
  sources: Dev[];
  latencyMs?: number | null;
  wiredPortIds?: Set<string>;
  portColor?: (portId: string) => string | undefined;
  onPortActivate?: (
    portId: string,
    kind: "src" | "sink",
    peerId: string,
    deviceId: string,
  ) => void;
  onConnectClick?: () => void;
  onDisconnect?: () => void;
};

function DevRow({
  peerId,
  dev,
  kind,
  side,
  wiredPortIds,
  portColor,
  onPortActivate,
}: {
  peerId: string;
  dev: Dev;
  kind: "src" | "sink";
  side: "left" | "right";
  wiredPortIds?: Set<string>;
  portColor?: (portId: string) => string | undefined;
  onPortActivate?: (
    portId: string,
    kind: "src" | "sink",
    peerId: string,
    deviceId: string,
  ) => void;
}) {
  const portId = `${peerId}:${kind}:${dev.id}`;
  const isLeft = side === "left";

  return (
    <div
      className={cn(
        "relative flex items-center gap-2.5 px-[11px] py-[5px] cursor-default",
        isLeft ? "justify-end pr-[22px] text-right" : "pl-[22px]",
      )}
    >
      <span className="text-[12px] text-ink-2">{dev.name}</span>
      <span
        className={cn(
          "absolute top-1/2 -translate-y-1/2",
          isLeft ? "right-[-7px]" : "left-[-7px]",
        )}
      >
        <Port
          peerId={peerId}
          kind={kind}
          deviceId={dev.id}
          wired={wiredPortIds?.has(portId)}
          color={portColor?.(portId)}
          onActivate={onPortActivate}
        />
      </span>
    </div>
  );
}

export function MachinePanel({
  peerId,
  name,
  side,
  isSelf,
  connected,
  sinks,
  sources,
  latencyMs,
  wiredPortIds,
  portColor,
  onPortActivate,
  onConnectClick,
  onDisconnect,
}: MachinePanelProps) {
  const isLeft = side === "left";

  if (side === "right" && !connected) {
    return (
      <div className="relative z-[2] w-[262px] bg-surface border border-line rounded-[3px]">
        <div className="py-[44px] px-5 text-center flex flex-col items-center gap-[5px]">
          <div className="w-[38px] h-[38px] rounded-[2px] border border-dashed border-line-2 text-ink-3 flex items-center justify-center text-[18px] mb-2">
            +
          </div>
          <button
            type="button"
            onClick={onConnectClick}
            className="mt-[13px] font-sans text-[11.5px] text-line bg-gold border-0 rounded-[2px] px-[15px] py-2 cursor-pointer font-semibold hover:brightness-110"
          >
            Conectar máquina
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="relative z-[2] w-[262px] bg-surface border border-line rounded-[3px]">
      <div className="flex items-center gap-[9px] px-[11px] py-[9px] bg-elev-1 border-b border-line">
        <span
          className={cn(
            "w-[7px] h-[7px] rounded-full shrink-0",
            connected ? "bg-green" : "bg-[#555]",
          )}
        />
        <span className="flex-1 min-w-0 truncate font-semibold text-[12.5px] tracking-[0.2px]">
          {name}
        </span>
        {!isSelf && (
          <div className="ml-auto flex items-center gap-2 shrink-0">
            {connected && latencyMs != null && (
              <span className="font-sans text-[10px] text-gold tabular-nums">
                {latencyMs} ms
              </span>
            )}
            <button
              type="button"
              title="desconectar"
              onClick={onDisconnect}
              className="font-sans text-[10px] text-ink-3 bg-elev-2 border border-line-2 rounded-[2px] px-2 py-[3px] cursor-pointer hover:text-gold hover:border-gold"
            >
              ✕
            </button>
          </div>
        )}
      </div>

      <div className="py-[7px] pb-[9px]">
        <div className="text-[8.5px] tracking-[1.4px] text-ink-3 font-semibold px-[11px] pt-[3px] pb-[7px]">
          DESTINOS
        </div>
        {sinks.map((dev) => (
          <DevRow
            key={dev.id}
            peerId={peerId}
            dev={dev}
            kind="sink"
            side={side}
            wiredPortIds={wiredPortIds}
            portColor={portColor}
            onPortActivate={onPortActivate}
          />
        ))}
      </div>

      <div className="h-px bg-line mx-[11px]" />

      <div className="py-[7px] pb-[9px]">
        <div className="text-[8.5px] tracking-[1.4px] text-ink-3 font-semibold px-[11px] pt-[3px] pb-[7px]">
          FONTES
        </div>
        {sources.map((dev) => (
          <DevRow
            key={dev.id}
            peerId={peerId}
            dev={dev}
            kind="src"
            side={side}
            wiredPortIds={wiredPortIds}
            portColor={portColor}
            onPortActivate={onPortActivate}
          />
        ))}
      </div>
    </div>
  );
}
