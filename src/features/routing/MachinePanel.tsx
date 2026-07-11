import { cn } from "@/lib/utils";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Port } from "./Port";
import { resolveConnection, type PortRef } from "./resolveConnection";

export const panelCardClass =
  "relative z-[2] w-[262px] rounded-xl border border-line bg-surface shadow-sm";

function SectionHeading({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[10.5px] text-ink-3 font-medium px-[11px] pt-[3px] pb-[7px]">
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
          <Button
            size="sm"
            onClick={onConnectClick}
            className="mt-[13px] text-[11.5px] text-line bg-gold font-semibold hover:brightness-110"
          >
            Conectar máquina
          </Button>
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
            <Button
              variant="outline"
              size="icon-xs"
              title="desconectar"
              onClick={onDisconnect}
              className="text-ink-2 border-line-2 hover:text-destructive hover:border-destructive"
            >
              ✕
            </Button>
          </div>
        )}
      </div>

      <div className="py-[7px] pb-[9px]">
        <SectionHeading>Destinos</SectionHeading>
        {sinks.map((dev) => (
          <DevRow
            key={dev.id}
            peerId={peerId}
            dev={dev}
            kind="sink"
            side={side}
            wiredPortIds={wiredPortIds}
            portColor={portColor}
            onDragStart={onDragStart}
            dragFrom={dragFrom}
            dragActive={dragActive}
          />
        ))}
      </div>

      <div className="h-px bg-line mx-[11px]" />

      <div className="py-[7px] pb-[9px]">
        <SectionHeading>Fontes</SectionHeading>
        {sources.map((dev) => (
          <DevRow
            key={dev.id}
            peerId={peerId}
            dev={dev}
            kind="src"
            side={side}
            wiredPortIds={wiredPortIds}
            portColor={portColor}
            onDragStart={onDragStart}
            dragFrom={dragFrom}
            dragActive={dragActive}
          />
        ))}
      </div>
    </Card>
  );
}
