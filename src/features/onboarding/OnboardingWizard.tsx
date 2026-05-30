import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { usePermissions, useRequestPermission } from "@/hooks/usePermissions";
import type { PermStatus } from "@/bindings";
import { useOnboarding } from "./useOnboarding";

type Step = "welcome" | "permissions" | "firewall" | "ready";
const STEPS: Step[] = ["welcome", "permissions", "firewall", "ready"];

function StepIndicator({ current }: { current: Step }) {
  const idx = STEPS.indexOf(current);
  return (
    <div className="flex gap-1.5 justify-center">
      {STEPS.map((s, i) => (
        <span
          key={s}
          className={`inline-block h-[3px] rounded-full transition-all ${
            i === idx
              ? "w-5 bg-gold"
              : i < idx
                ? "w-3 bg-gold/40"
                : "w-3 bg-[#3a3a3e]"
          }`}
        />
      ))}
    </div>
  );
}

function permissionsGrantedOrNA(mic: PermStatus, screen: PermStatus) {
  const ok = (s: PermStatus) => s === "granted" || s === "not_applicable";
  return ok(mic) && ok(screen);
}

function PermissionRow({
  label,
  status,
  kind,
}: {
  label: string;
  status: PermStatus;
  kind: "microphone" | "screen";
}) {
  const request = useRequestPermission();
  const needsRequest = status !== "granted" && status !== "not_applicable";

  const statusLabel: Record<PermStatus, string> = {
    granted: "Permitido",
    denied: "Negado",
    prompt: "Pendente",
    not_applicable: "N/A",
  };

  return (
    <div className="flex items-center justify-between py-[7px] px-[11px] rounded-[2px] bg-[#26262a]">
      <div className="flex flex-col gap-0.5">
        <span className="text-[12.5px] text-ink">{label}</span>
        <span
          className={`font-mono text-[10px] ${
            status === "granted"
              ? "text-green"
              : status === "denied"
                ? "text-red-400"
                : "text-ink-3"
          }`}
        >
          {statusLabel[status]}
        </span>
      </div>
      {needsRequest && (
        <button
          type="button"
          onClick={() => request.mutate(kind)}
          disabled={request.isPending}
          className="font-mono text-[11px] text-ink-2 bg-[#242426] border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:text-ink hover:border-line disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Permitir
        </button>
      )}
    </div>
  );
}

function WelcomeStep() {
  return (
    <div className="flex flex-col gap-3">
      <p className="text-[13px] text-ink leading-relaxed">
        Splitter compartilha áudio entre PCs na sua rede local.
      </p>
      <p className="text-[12px] text-ink-2 leading-relaxed">
        Em poucos passos você configura as permissões necessárias e o Splitter
        estará pronto para uso.
      </p>
    </div>
  );
}

function PermissionsStep({ onSkip }: { onSkip: () => void }) {
  const { data: permissions } = usePermissions();

  const mic = permissions?.microphone ?? "not_applicable";
  const screen = permissions?.screen ?? "not_applicable";
  const allNA = mic === "not_applicable" && screen === "not_applicable";

  if (allNA) {
    return (
      <div className="flex flex-col gap-3">
        <p className="text-[12px] text-ink-2">
          Nenhuma permissão necessária nesta plataforma.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <p className="text-[12px] text-ink-2 mb-1">
        Permissões necessárias para capturar áudio.
      </p>
      {mic !== "not_applicable" && (
        <PermissionRow label="Microfone" status={mic} kind="microphone" />
      )}
      {screen !== "not_applicable" && (
        <PermissionRow label="Áudio do sistema" status={screen} kind="screen" />
      )}
      <div className="flex justify-end mt-1">
        <button
          type="button"
          onClick={onSkip}
          className="font-mono text-[11px] text-ink-3 hover:text-ink-2 cursor-pointer"
        >
          Pular
        </button>
      </div>
    </div>
  );
}

function FirewallStep() {
  return (
    <div className="flex flex-col gap-3">
      <p className="text-[12px] text-ink-2 leading-relaxed">
        A rede local precisa permitir as portas de sinalização (TCP) e áudio
        (UDP). Em redes domésticas geralmente funciona sem ajustes.
      </p>
    </div>
  );
}

function ReadyStep({ onComplete }: { onComplete: () => void }) {
  return (
    <div className="flex flex-col gap-4 items-center text-center">
      <p className="text-[13px] text-ink">Splitter está pronto para uso.</p>
      <Button
        onClick={onComplete}
        className="font-mono text-[11px] bg-gold text-[#1c1c1f] hover:bg-gold/90 rounded-[2px] px-5 h-8"
      >
        Concluir
      </Button>
    </div>
  );
}

export function OnboardingWizard() {
  const onboarded = useOnboarding((s) => s.onboarded);
  const complete = useOnboarding((s) => s.complete);
  const [step, setStep] = useState<Step>("welcome");
  const { data: permissions } = usePermissions();

  if (onboarded) return null;

  const idx = STEPS.indexOf(step);

  function next() {
    setStep(STEPS[idx + 1]);
  }

  function back() {
    setStep(STEPS[idx - 1]);
  }

  const mic = permissions?.microphone ?? "not_applicable";
  const screen = permissions?.screen ?? "not_applicable";
  const canAdvancePermissions = permissionsGrantedOrNA(mic, screen);

  const stepTitles: Record<Step, string> = {
    welcome: "Bem-vindo ao Splitter",
    permissions: "Permissões",
    firewall: "Rede",
    ready: "Pronto",
  };

  return (
    <Dialog open modal>
      <DialogContent
        showCloseButton={false}
        onInteractOutside={(e) => e.preventDefault()}
        onEscapeKeyDown={(e) => e.preventDefault()}
        aria-describedby={undefined}
        className="w-[400px] max-w-[400px] bg-surface border-line rounded-[3px] gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-[#2a2a2d] border-b border-line rounded-t-[3px]">
          <DialogTitle className="font-mono text-[9.5px] tracking-[0.5px] text-ink-3 font-semibold uppercase">
            {stepTitles[step]}
          </DialogTitle>
        </DialogHeader>

        <div className="px-[15px] py-[14px] min-h-[140px]">
          {step === "welcome" && <WelcomeStep />}
          {step === "permissions" && <PermissionsStep onSkip={next} />}
          {step === "firewall" && <FirewallStep />}
          {step === "ready" && <ReadyStep onComplete={complete} />}
        </div>

        <div className="px-[13px] py-[9px] border-t border-line flex items-center justify-between gap-2">
          <StepIndicator current={step} />

          <div className="flex gap-2">
            {idx > 0 && step !== "ready" && (
              <button
                type="button"
                onClick={back}
                className="font-mono text-[11px] text-ink-2 bg-[#242426] border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:text-ink hover:border-line"
              >
                Voltar
              </button>
            )}
            {step !== "ready" && (
              <button
                type="button"
                onClick={next}
                disabled={step === "permissions" && !canAdvancePermissions}
                className="font-mono text-[11px] text-ink bg-[#2e2e32] border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:border-line disabled:opacity-40 disabled:cursor-not-allowed"
              >
                Próximo
              </button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
