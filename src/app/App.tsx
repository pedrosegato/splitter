import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import { queryClient } from "@/app/queryClient";
import { mountEventBridge } from "@/lib/events";

export function App() {
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    mountEventBridge(queryClient).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  return (
    <div className="bg-board h-full flex flex-col items-center justify-center gap-4">
      <h1 className="text-accent text-2xl font-semibold tracking-wide">
        Splitter
      </h1>
      <p className="text-ink-2 text-sm">Audio routing — UI loading…</p>
      <Button variant="outline">Open session</Button>
    </div>
  );
}
