import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AudioLines, Minus, Settings, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";

import { queryClient } from "@/app/queryClient";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { IncomingRequestDialog } from "@/features/connect/IncomingRequestDialog";
import { OnboardingWizard } from "@/features/onboarding/OnboardingWizard";
import { RoutingBoard } from "@/features/routing/RoutingBoard";
import { SettingsDialog } from "@/features/settings/SettingsDialog";
import { StatsView } from "@/features/stats/StatsView";
import { mountEventBridge } from "@/lib/events";
import { variants } from "@/lib/motion";
import { useUiStore } from "@/stores/ui";

const isMac = typeof navigator !== "undefined" && /Macintosh|Mac OS X/i.test(navigator.userAgent);

export function App() {
  const activeTab = useUiStore((s) => s.activeTab);
  const setTab = useUiStore((s) => s.setTab);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    mountEventBridge(queryClient).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return (
    <div className="flex h-full flex-col font-sans">
      <header
        data-tauri-drag-region
        className={`border-line bg-elev-0 flex h-11 shrink-0 items-center border-b ${
          isMac ? "pr-4 pl-[82px]" : "pr-0 pl-3"
        }`}
      >
        <div data-tauri-drag-region className="flex items-center gap-1.5">
          <AudioLines size={18} className="text-gold shrink-0" />
          <span className="text-ink text-xs font-semibold tracking-wide">Splitter</span>
        </div>
        <div className="ml-auto flex items-center gap-3">
          <Tabs value={activeTab} onValueChange={(v) => setTab(v as "routing" | "stats")}>
            <TabsList className="bg-surface h-8 gap-0 rounded-md p-[3px]">
              <TabsTrigger
                value="routing"
                className="data-[state=active]:bg-surface-2 data-[state=active]:text-ink data-[state=inactive]:text-ink-3 px-3 text-xs data-[state=active]:shadow-none"
              >
                Roteamento
              </TabsTrigger>
              <TabsTrigger
                value="stats"
                className="data-[state=active]:bg-surface-2 data-[state=active]:text-ink data-[state=inactive]:text-ink-3 px-3 text-xs data-[state=active]:shadow-none"
              >
                Estatísticas
              </TabsTrigger>
            </TabsList>
          </Tabs>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Configurações"
            onClick={() => setSettingsOpen(true)}
            className="text-ink-2 hover:text-ink"
          >
            <Settings size={16} />
          </Button>
        </div>
        {!isMac && (
          <div className="ml-2 flex h-full items-stretch">
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Minimizar"
              onClick={() => getCurrentWindow().minimize()}
              className="text-ink-2 hover:bg-elev-2 hover:text-ink h-full w-[46px]"
            >
              <Minus size={15} />
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Maximizar"
              onClick={() => getCurrentWindow().toggleMaximize()}
              className="text-ink-2 hover:bg-elev-2 hover:text-ink h-full w-[46px]"
            >
              <Square size={12} />
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Fechar"
              onClick={() => getCurrentWindow().close()}
              className="text-ink-2 h-full w-[46px] hover:bg-[#e81123] hover:text-white"
            >
              <X size={16} />
            </Button>
          </div>
        )}
      </header>
      <main className="bg-board flex-1 overflow-auto">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={activeTab}
            variants={variants.fadeInUp}
            initial="hidden"
            animate="show"
            exit={{ opacity: 0, y: -6 }}
            className="h-full overflow-auto"
          >
            {activeTab === "routing" ? <RoutingBoard /> : <StatsView />}
          </motion.div>
        </AnimatePresence>
      </main>
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
      <OnboardingWizard />
      <IncomingRequestDialog />
      <Toaster position="bottom-right" />
    </div>
  );
}
