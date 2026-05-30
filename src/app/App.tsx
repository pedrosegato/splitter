import { useEffect, useState } from "react";
import { queryClient } from "@/app/queryClient";
import { mountEventBridge } from "@/lib/events";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useUiStore } from "@/stores/ui";
import { RoutingBoard } from "@/features/routing/RoutingBoard";
import { StatsView } from "@/features/stats/StatsView";
import { SettingsDialog } from "@/features/settings/SettingsDialog";
import { OnboardingWizard } from "@/features/onboarding/OnboardingWizard";
import { AudioLines, Settings } from "lucide-react";
import { Toaster } from "@/components/ui/sonner";

export function App() {
  const activeTab = useUiStore((s) => s.activeTab);
  const setTab = useUiStore((s) => s.setTab);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    mountEventBridge(queryClient).then((fn) => {
      if (cancelled) { fn(); } else { unlisten = fn; }
    });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  return (
    <div className="h-full flex flex-col font-sans">
      <header
        data-tauri-drag-region
        className="h-11 flex items-center pl-[82px] pr-4 border-b border-line shrink-0 bg-elev-0"
      >
        <div data-tauri-drag-region className="flex items-center gap-1.5">
          <AudioLines size={18} className="text-gold shrink-0" />
          <span className="text-xs font-semibold tracking-wide text-ink">Splitter</span>
        </div>
        <div className="ml-auto flex items-center gap-3">
          <Tabs
            value={activeTab}
            onValueChange={(v) => setTab(v as "routing" | "stats")}
          >
            <TabsList
              className="h-8 gap-0 rounded-md p-[3px] bg-surface"
            >
              <TabsTrigger
                value="routing"
                className="rounded-sm px-3 text-xs data-[state=active]:bg-surface-2 data-[state=active]:text-ink data-[state=inactive]:text-ink-3 data-[state=active]:shadow-none"
              >
                Roteamento
              </TabsTrigger>
              <TabsTrigger
                value="stats"
                className="rounded-sm px-3 text-xs data-[state=active]:bg-surface-2 data-[state=active]:text-ink data-[state=inactive]:text-ink-3 data-[state=active]:shadow-none"
              >
                Estatísticas
              </TabsTrigger>
            </TabsList>
          </Tabs>
          <button
            type="button"
            aria-label="Configurações"
            onClick={() => setSettingsOpen(true)}
            className="flex items-center justify-center w-6 h-6 rounded-sm text-ink-2 hover:text-ink focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-gold"
          >
            <Settings size={16} />
          </button>
        </div>
      </header>
      <main className="flex-1 overflow-auto bg-board">
        {activeTab === "routing" ? <RoutingBoard /> : <StatsView />}
      </main>
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
      <OnboardingWizard />
      <Toaster position="bottom-right" />
    </div>
  );
}
