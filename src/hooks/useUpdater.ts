import { useState, useCallback } from "react";
import { toast } from "sonner";

type UpdateState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "available"; version: string; onInstall: () => Promise<void> }
  | { status: "installing" }
  | { status: "error"; message: string };

export function useUpdater() {
  const [state, setState] = useState<UpdateState>({ status: "idle" });

  const checkForUpdates = useCallback(async () => {
    setState({ status: "checking" });
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const { relaunch } = await import("@tauri-apps/plugin-process");

      const update = await check();

      if (!update) {
        setState({ status: "idle" });
        toast.success("Nenhuma atualização disponível");
        return;
      }

      const onInstall = async () => {
        setState({ status: "installing" });
        try {
          await update.downloadAndInstall();
          await relaunch();
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          setState({ status: "error", message });
          toast.error(`Falha ao instalar: ${message}`);
        }
      };

      setState({ status: "available", version: update.version, onInstall });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (message.toLowerCase().includes("dev") || message.toLowerCase().includes("no such host") || message.toLowerCase().includes("failed to fetch")) {
        setState({ status: "idle" });
        toast.info("Verificação de atualização indisponível em modo de desenvolvimento");
      } else {
        setState({ status: "error", message });
        toast.error(`Erro ao verificar atualizações: ${message}`);
      }
    }
  }, []);

  return { state, checkForUpdates };
}
