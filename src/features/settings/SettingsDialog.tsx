import { useCallback, useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useSettingsForm } from "./useSettingsForm";
import { useThemeStore, applyTheme } from "@/stores/theme";
import { useUpdater } from "@/hooks/useUpdater";
import { useIdentity } from "@/hooks/useIdentity";
import { useSetDeviceName } from "@/hooks/useDeviceName";
import { useResetSettings } from "@/hooks/useSettings";

function AppVersion() {
  const [v, setV] = useState("");
  useEffect(() => {
    import("@tauri-apps/api/app").then((m) => m.getVersion()).then(setV).catch(() => setV("?"));
  }, []);
  return <span className="text-[11px] text-ink-2">{v || "…"}</span>;
}

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
};

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-[11px] text-ink-3 font-medium mb-[8px] mt-[14px] first:mt-0">
      {children}
    </p>
  );
}

function Row({ children }: { children: React.ReactNode }) {
  return (
    <Field
      orientation="horizontal"
      className="gap-4 py-[7px] px-[11px] hover:bg-elev-2"
    >
      {children}
    </Field>
  );
}

function SettingLabel({ htmlFor, children }: { htmlFor?: string; children: React.ReactNode }) {
  return (
    <FieldLabel htmlFor={htmlFor} className="text-[12.5px] text-ink cursor-default">
      {children}
    </FieldLabel>
  );
}

function useDebouncedSetter(set: (key: string, value: string | number | boolean) => void, delay = 300) {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  return useCallback(
    (key: string, value: string | number | boolean) => {
      if (timerRef.current !== null) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => set(key, value), delay);
    },
    [set, delay],
  );
}

const inputClass = "h-[28px] text-[12px] bg-board border-line-2 text-ink focus-visible:ring-gold focus-visible:border-gold";

function SettingSelect({
  id,
  value,
  onValueChange,
  options,
}: {
  id?: string;
  value: string;
  onValueChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <Select value={value} onValueChange={onValueChange}>
      <SelectTrigger
        id={id}
        size="sm"
        className="w-[110px] h-[28px] text-[12px] bg-board border-line-2 text-ink focus-visible:ring-gold"
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function NumberInput({
  id,
  settingKey,
  value,
  min,
  max,
  set,
}: {
  id: string;
  settingKey: string;
  value: number;
  min: number;
  max: number;
  set: (key: string, value: string | number | boolean) => void;
}) {
  const [local, setLocal] = useState(String(value));
  const debouncedSet = useDebouncedSetter(set);

  useEffect(() => {
    setLocal(String(value));
  }, [value]);

  return (
    <Input
      id={id}
      type="number"
      min={min}
      max={max}
      value={local}
      onChange={(e) => {
        setLocal(e.target.value);
        const n = Number(e.target.value);
        if (!Number.isNaN(n)) debouncedSet(settingKey, n);
      }}
      className={`w-[90px] ${inputClass}`}
    />
  );
}

type JitterModeString = "auto" | "min" | "fixed";

function parseJitterMode(mode: { fixed: number } | "auto" | "min"): { base: JitterModeString; fixedMs: number } {
  if (mode === "auto") return { base: "auto", fixedMs: 40 };
  if (mode === "min") return { base: "min", fixedMs: 40 };
  return { base: "fixed", fixedMs: mode.fixed };
}

export function SettingsDialog({ open, onOpenChange }: Props) {
  const { settings, isLoading, isSaved, set, setAutostart } = useSettingsForm();
  const { theme, setTheme } = useThemeStore();
  const { state: updateState, checkForUpdates } = useUpdater();
  const { data: identity } = useIdentity();
  const setDeviceName = useSetDeviceName();
  const resetSettings = useResetSettings();
  const [nameDraft, setNameDraft] = useState("");

  useEffect(() => {
    if (identity) setNameDraft(identity.peer_name);
  }, [identity]);

  const commitName = () => {
    if (identity && nameDraft.trim() && nameDraft !== identity.peer_name) {
      setDeviceName.mutate(nameDraft);
    }
  };

  const handleReset = () => {
    if (
      window.confirm(
        "Restaurar todas as configurações para o padrão? O nome do dispositivo não será alterado.",
      )
    ) {
      resetSettings.mutate(undefined, { onSuccess: () => setAutostart(false) });
    }
  };

  const [savedVisible, setSavedVisible] = useState(false);
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (isSaved) {
      setSavedVisible(true);
      if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current);
      savedTimerRef.current = setTimeout(() => setSavedVisible(false), 1500);
    }
  }, [isSaved]);

  const jitter = settings ? parseJitterMode(settings.jitter_mode) : { base: "auto" as JitterModeString, fixedMs: 40 };

  if (isLoading || !settings) return null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[420px] max-w-[420px] bg-surface border-line gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-elev-1 border-b border-line rounded-t-lg flex-row items-center justify-between">
          <DialogTitle className="text-[11px] text-ink-3 font-medium">
            Configurações
          </DialogTitle>
          {savedVisible && (
            <Badge variant="secondary" className="text-[9.5px] text-gold tracking-wide">
              salvo
            </Badge>
          )}
        </DialogHeader>

        <div className="px-[11px] py-[11px] overflow-y-auto max-h-[520px]">
          <SectionLabel>Dispositivo</SectionLabel>

          <FieldGroup className="gap-0">
            <Row>
              <SettingLabel htmlFor="device-name">Nome do dispositivo</SettingLabel>
              <Input
                id="device-name"
                aria-label="Nome do dispositivo"
                value={nameDraft}
                maxLength={40}
                onChange={(e) => setNameDraft(e.target.value)}
                onBlur={commitName}
                onKeyDown={(e) => {
                  if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                }}
                className={`w-[170px] ${inputClass}`}
              />
            </Row>
          </FieldGroup>

          <SectionLabel>Conexão</SectionLabel>

          <FieldGroup className="gap-0">
            <Row>
              <SettingLabel htmlFor="auto-accept-trusted">
                Aceitar conexões automaticamente
              </SettingLabel>
              <Switch
                id="auto-accept-trusted"
                size="sm"
                checked={settings.auto_accept_trusted}
                onCheckedChange={(checked) => set("auto_accept_trusted", checked)}
              />
            </Row>

            <Row>
              <SettingLabel htmlFor="signaling-port">Porta de sinalização</SettingLabel>
              <NumberInput
                id="signaling-port"
                settingKey="signaling_port"
                value={settings.signaling_port}
                min={1024}
                max={65535}
                set={set}
              />
            </Row>
          </FieldGroup>

          <SectionLabel>Áudio</SectionLabel>

          <FieldGroup className="gap-0">
            <Row>
              <SettingLabel>Bitrate padrão</SettingLabel>
              <SettingSelect
                value={String(settings.default_bitrate)}
                onValueChange={(v) => set("default_bitrate", Number(v))}
                options={[
                  { value: "64000", label: "64 kbps" },
                  { value: "96000", label: "96 kbps" },
                  { value: "128000", label: "128 kbps" },
                ]}
              />
            </Row>

            <Row>
              <SettingLabel>Modo FEC</SettingLabel>
              <SettingSelect
                value={settings.fec_mode}
                onValueChange={(v) => set("fec_mode", v)}
                options={[
                  { value: "auto", label: "auto" },
                  { value: "always", label: "sempre" },
                  { value: "never", label: "nunca" },
                ]}
              />
            </Row>

            <Row>
              <SettingLabel>Modo jitter</SettingLabel>
              <SettingSelect
                value={jitter.base}
                onValueChange={(v) => {
                  if (v === "auto" || v === "min") {
                    set("jitter_mode", v);
                  } else {
                    set("jitter_mode", `fixed:${jitter.fixedMs}`);
                  }
                }}
                options={[
                  { value: "auto", label: "auto" },
                  { value: "min", label: "min" },
                  { value: "fixed", label: "fixo" },
                ]}
              />
            </Row>

            {jitter.base === "fixed" && (
              <Row>
                <SettingLabel htmlFor="jitter-fixed-ms">Jitter fixo (ms)</SettingLabel>
                <NumberInput
                  id="jitter-fixed-ms"
                  settingKey="jitter_mode"
                  value={jitter.fixedMs}
                  min={0}
                  max={500}
                  set={(_, v) => set("jitter_mode", `fixed:${v}`)}
                />
              </Row>
            )}

            <Row>
              <SettingLabel htmlFor="jitter-max-depth">Profundidade máxima jitter (ms)</SettingLabel>
              <NumberInput
                id="jitter-max-depth"
                settingKey="jitter_max_depth_ms"
                value={settings.jitter_max_depth_ms}
                min={0}
                max={1000}
                set={set}
              />
            </Row>
          </FieldGroup>

          <SectionLabel>Sistema</SectionLabel>

          <FieldGroup className="gap-0">
            <Row>
              <SettingLabel htmlFor="auto-start-system">Iniciar com o sistema</SettingLabel>
              <Switch
                id="auto-start-system"
                size="sm"
                checked={settings.auto_start_with_system}
                onCheckedChange={(checked) => setAutostart(checked)}
              />
            </Row>

            <Row>
              <SettingLabel>Nível de log</SettingLabel>
              <SettingSelect
                value={settings.log_level}
                onValueChange={(v) => set("log_level", v)}
                options={[
                  { value: "trace", label: "trace" },
                  { value: "debug", label: "debug" },
                  { value: "info", label: "info" },
                  { value: "warn", label: "warn" },
                  { value: "error", label: "error" },
                ]}
              />
            </Row>

            <Row>
              <SettingLabel htmlFor="metrics-enabled">Métricas habilitadas</SettingLabel>
              <Switch
                id="metrics-enabled"
                size="sm"
                checked={settings.metrics_enabled}
                onCheckedChange={(checked) => set("metrics_enabled", checked)}
              />
            </Row>
          </FieldGroup>

          <SectionLabel>Aparência</SectionLabel>

          <FieldGroup className="gap-0">
            <Row>
              <SettingLabel>Tema</SettingLabel>
              <ToggleGroup
                type="single"
                value={theme}
                onValueChange={(v) => {
                  if (v === "dark" || v === "light") {
                    setTheme(v);
                    applyTheme(v);
                  }
                }}
                size="sm"
                className="border border-line-2 overflow-hidden"
              >
                <ToggleGroupItem
                  value="dark"
                  className="px-[10px] text-[11px] bg-board text-ink-2 hover:bg-board hover:text-ink data-[state=on]:bg-gold data-[state=on]:text-[#1c1c1f] data-[state=on]:font-semibold"
                >
                  Escuro
                </ToggleGroupItem>
                <ToggleGroupItem
                  value="light"
                  className="px-[10px] text-[11px] border-l border-line-2 bg-board text-ink-2 hover:bg-board hover:text-ink data-[state=on]:bg-gold data-[state=on]:text-[#1c1c1f] data-[state=on]:font-semibold"
                >
                  Claro
                </ToggleGroupItem>
              </ToggleGroup>
            </Row>
          </FieldGroup>

          <SectionLabel>Sobre</SectionLabel>

          <FieldGroup className="gap-0">
            <Row>
              <SettingLabel>Versão</SettingLabel>
              <AppVersion />
            </Row>

            <Row>
              <SettingLabel>Atualizações</SettingLabel>
              {updateState.status === "available" ? (
                <div className="flex items-center gap-2">
                  <Badge variant="secondary" className="text-[10px] text-gold">
                    v{updateState.version}
                  </Badge>
                  <Button
                    size="sm"
                    onClick={updateState.onInstall}
                    className="text-[11px] bg-gold text-[#1c1c1f] hover:bg-gold/90"
                  >
                    instalar
                  </Button>
                </div>
              ) : (
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={checkForUpdates}
                  disabled={updateState.status === "checking" || updateState.status === "installing"}
                  className="text-[11px]"
                >
                  {updateState.status === "checking"
                    ? "verificando…"
                    : updateState.status === "installing"
                    ? "instalando…"
                    : "buscar atualizações"}
                </Button>
              )}
            </Row>
          </FieldGroup>
        </div>

        <div className="flex items-center justify-between px-[13px] py-[9px] border-t border-line">
          <Button
            variant="outline"
            size="sm"
            onClick={handleReset}
            className="text-[11px] text-ink-3 hover:text-gold hover:border-gold"
          >
            Restaurar padrões
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => onOpenChange(false)}
            className="text-[11px]"
          >
            fechar
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
