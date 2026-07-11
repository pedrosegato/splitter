import { cn } from "@/lib/utils";
import { Card } from "@/components/ui/card";
import { Port } from "./Port";
import { resolveConnection, type PortRef } from "./resolveConnection";

export const panelCardClass =
  "relative z-[2] w-[262px] rounded-xl border border-line bg-surface shadow-sm";

function SectionHeading({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[8.5px] tracking-[1.4px] text-ink-3 font-semibold px-[11px] pt-[3px] pb-[7px]">
      {children}
    </div>
  );
}

type Dev = { id: string; name: string };

type MachinePanelProps = {
  peerId: string;
  name: string;
  side: "left" | "right";
  isSelf?: boolean;
  connected: boolean;
  sinks: Dev[];
  sources: Dev[];
  wiredPortIds?: Set<string>;
  portColor?: (portId: string) => string | undefined;
  onPortActivate?: (
    portId: string,
    kind: "src" | "sink",
    peerId: string,
    deviceId: string,
  ) => void;
  onDragStart?: (ref: PortRef, e: React.PointerEvent) => void;
  dragFrom?: PortRef | null;
  dragActive?: boolean;
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
  onDragStart,
  dragFrom,
  dragActive,
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
  onDragStart?: (ref: PortRef, e: React.PointerEvent) => void;
  dragFrom?: PortRef | null;
  dragActive?: boolean;
}) {
  const portId = `${peerId}:${kind}:${dev.id}`;
  const isLeft = side === "left";

  const thisRef: PortRef = { peerId, deviceId: dev.id, kind };
  const isOrigin =
    !!dragFrom &&
    dragFrom.peerId === thisRef.peerId &&
    dragFrom.deviceId === thisRef.deviceId &&
    dragFrom.kind === thisRef.kind;
  const highlighted =
    !!dragActive && !!dragFrom && resolveConnection(dragFrom, thisRef) !== null;
  const dimmed = !!dragActive && !!dragFrom && !isOrigin && !highlighted;

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
          onDragStart={onDragStart}
          highlighted={highlighted}
          dimmed={dimmed}
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
  wiredPortIds,
  portColor,
  onPortActivate,
  onDragStart,
  dragFrom,
  dragActive,
  onConnectClick,
  onDisconnect,
}: MachinePanelProps) {
  const isLeft = side === "left";

  if (side === "right" && !connected) {
    return (
      <Card className={cn(panelCardClass, "gap-0 py-0")}>
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
      </Card>
    );
  }

  return (
    <Card className={cn(panelCardClass, "gap-0 py-0")}>
      <div className="flex items-center gap-[9px] px-[11px] py-[9px] bg-elev-1 border-b border-line rounded-t-xl">
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
        <SectionHeading>DESTINOS</SectionHeading>
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
            onDragStart={onDragStart}
            dragFrom={dragFrom}
            dragActive={dragActive}
          />
        ))}
      </div>

      <div className="h-px bg-line mx-[11px]" />

      <div className="py-[7px] pb-[9px]">
        <SectionHeading>FONTES</SectionHeading>
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
            onDragStart={onDragStart}
            dragFrom={dragFrom}
            dragActive={dragActive}
          />
        ))}
      </div>
    </Card>
  );
}
