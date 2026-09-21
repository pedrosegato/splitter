import { useState } from "react";
import { AnimatePresence, motion } from "motion/react";

import type { PermStatus } from "@/bindings";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { usePermissions, useRequestPermission } from "@/hooks/usePermissions";
import { variants } from "@/lib/motion";

import { useOnboarding } from "./useOnboarding";

type Step = "welcome" | "permissions" | "firewall" | "ready";
const STEPS: Step[] = ["welcome", "permissions", "firewall", "ready"];

function StepIndicator({ current }: { current: Step }) {
  const idx = STEPS.indexOf(current);
  return (
    <div className="flex justify-center gap-1.5">
      {STEPS.map((s, i) =>
        i === idx ? (
          <motion.span
            key={s}
            layoutId="wizard-dot"
            className="bg-gold inline-block h-[3px] w-5 rounded-full"
          />
        ) : (
          <span
            key={s}
            className={`inline-block h-[3px] rounded-full transition-all ${
              i < idx ? "bg-gold/40 w-3" : "bg-surface-2 w-3"
            }`}
          />
        ),
      )}
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
    <div className="bg-elev-2 flex items-center justify-between px-[11px] py-[7px]">
      <div className="flex flex-col gap-0.5">
        <span className="text-ink text-[12.5px]">{label}</span>
        <Badge
          variant="secondary"
          className={`text-[10px] ${
            status === "granted"
              ? "text-green"
              : status === "denied"
                ? "text-red-400"
                : "text-ink-3"
          }`}
        >
          {statusLabel[status]}
        </Badge>
      </div>
      {needsRequest && (
        <Button
          size="sm"
          onClick={() => request.mutate(kind)}
          disabled={request.isPending}
          className="text-[11px]"
        >
          Permitir
        </Button>
      )}
    </div>
  );
}

function WelcomeStep() {
  return (
    <div className="flex flex-col gap-3">
      <p className="text-ink text-[13px] leading-relaxed">
        Splitter compartilha áudio entre PCs na sua rede local.
      </p>
      <p className="text-ink-2 text-[12px] leading-relaxed">
        Em poucos passos você configura as permissões necessárias e o Splitter estará pronto para
        uso.
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
        <p className="text-ink-2 text-[12px]">Nenhuma permissão necessária nesta plataforma.</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <p className="text-ink-2 mb-1 text-[12px]">Permissões necessárias para capturar áudio.</p>
      {mic !== "not_applicable" && (
        <PermissionRow label="Microfone" status={mic} kind="microphone" />
      )}
      {screen !== "not_applicable" && (
        <PermissionRow label="Áudio do sistema" status={screen} kind="screen" />
      )}
      <div className="mt-1 flex justify-end">
        <Button
          variant="ghost"
          size="sm"
          onClick={onSkip}
          className="text-ink-3 hover:text-ink-2 text-[11px]"
        >
          Pular
        </Button>
      </div>
    </div>
  );
}

function FirewallStep() {
  return (
    <div className="flex flex-col gap-3">
      <p className="text-ink-2 text-[12px] leading-relaxed">
        A rede local precisa permitir as portas de sinalização (TCP) e áudio (UDP). Em redes
        domésticas geralmente funciona sem ajustes.
      </p>
    </div>
  );
}

function ReadyStep({ onComplete }: { onComplete: () => void }) {
  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <p className="text-ink text-[13px]">Splitter está pronto para uso.</p>
      <Button
        onClick={onComplete}
        className="bg-gold hover:bg-gold/90 h-8 px-5 text-[11px] text-[#1c1c1f]"
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
  const [direction, setDirection] = useState<1 | -1>(1);
  const { data: permissions } = usePermissions();

  if (onboarded) return null;

  const idx = STEPS.indexOf(step);

  function next() {
    setDirection(1);
    setStep(STEPS[idx + 1]);
  }

  function back() {
    setDirection(-1);
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
        className="bg-surface border-line w-[400px] max-w-[400px] gap-0 p-0"
      >
        <DialogHeader className="bg-elev-1 border-line rounded-t-lg border-b px-[15px] py-3">
          <DialogTitle className="text-ink-3 text-[11px] font-medium">
            {stepTitles[step]}
          </DialogTitle>
        </DialogHeader>

        <div className="min-h-[140px] overflow-hidden px-[15px] py-[14px]">
          <AnimatePresence mode="wait" custom={direction}>
            <motion.div
              key={step}
              custom={direction}
              variants={variants.slide(direction)}
              initial="enter"
              animate="center"
              exit="exit"
            >
              {step === "welcome" && <WelcomeStep />}
              {step === "permissions" && <PermissionsStep onSkip={next} />}
              {step === "firewall" && <FirewallStep />}
              {step === "ready" && <ReadyStep onComplete={complete} />}
            </motion.div>
          </AnimatePresence>
        </div>

        <div className="border-line flex items-center justify-between gap-2 border-t px-[13px] py-[9px]">
          <StepIndicator current={step} />

          <div className="flex gap-2">
            {idx > 0 && step !== "ready" && (
              <Button variant="secondary" size="sm" onClick={back} className="text-[11px]">
                Voltar
              </Button>
            )}
            {step !== "ready" && (
              <Button
                size="sm"
                onClick={next}
                disabled={step === "permissions" && !canAdvancePermissions}
                className="text-[11px]"
              >
                Próximo
              </Button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
