import { useEffect, useState } from "react";
import { queryClient } from "@/app/queryClient";
import { mountEventBridge } from "@/lib/events";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useUiStore } from "@/stores/ui";
import { useSnapshot } from "@/hooks/useSnapshot";
import { RoutingBoard } from "@/features/routing/RoutingBoard";
import { StatsView } from "@/features/stats/StatsView";
import { SettingsDialog } from "@/features/settings/SettingsDialog";
import { OnboardingWizard } from "@/features/onboarding/OnboardingWizard";
import { AudioLines, Settings } from "lucide-react";
import { Toaster } from "@/components/ui/sonner";

function StatusDot() {
  const { data: sessions } = useSnapshot();
  const active = (sessions?.length ?? 0) > 0;

  return (
    <span
      className={`inline-block w-[7px] h-[7px] rounded-full ${active ? "bg-green" : "bg-[#555]"}`}
    />
  );
}

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
        className="h-10 flex items-center px-3.5 border-b border-line shrink-0 bg-elev-0"
      >
        <div className="flex items-center gap-1.5">
          <AudioLines size={18} className="text-gold shrink-0" />
          <span className="text-xs font-semibold tracking-wide text-ink">Splitter</span>
          <StatusDot />
        </div>
        <div className="ml-auto flex items-center gap-2">
          <Tabs
            value={activeTab}
            onValueChange={(v) => setTab(v as "routing" | "stats")}
          >
            <TabsList
              className="h-7 gap-0 rounded-sm p-0 bg-surface"
            >
              <TabsTrigger
                value="routing"
                className="h-7 rounded-sm px-3 text-xs data-[state=active]:bg-surface-2 data-[state=active]:text-ink data-[state=inactive]:text-ink-3 data-[state=active]:shadow-none"
              >
                Roteamento
              </TabsTrigger>
              <TabsTrigger
                value="stats"
                className="h-7 rounded-sm px-3 text-xs data-[state=active]:bg-surface-2 data-[state=active]:text-ink data-[state=inactive]:text-ink-3 data-[state=active]:shadow-none"
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
