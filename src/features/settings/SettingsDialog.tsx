import { useCallback, useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
  return <span className="font-mono text-[11px] text-ink-2">{v || "…"}</span>;
}

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
};

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="font-mono text-[9.5px] tracking-[0.5px] text-ink-3 font-semibold uppercase mb-[8px] mt-[14px] first:mt-0">
      {children}
    </p>
  );
}

function Row({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 py-[7px] px-[11px] rounded-[2px] hover:bg-elev-2">
      {children}
    </div>
  );
}

function SettingLabel({ htmlFor, children }: { htmlFor?: string; children: React.ReactNode }) {
  return (
    <Label htmlFor={htmlFor} className="text-[12.5px] text-ink cursor-default">
      {children}
    </Label>
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
        className="w-[110px] h-[28px] text-[12px] font-mono bg-board border-line-2 text-ink focus-visible:ring-gold"
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

function ThemeButton({
  value,
  active,
  onSelect,
  label,
  borderLeft,
}: {
  value: "dark" | "light";
  active: boolean;
  onSelect: (value: "dark" | "light") => void;
  label: string;
  borderLeft?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={() => onSelect(value)}
      className={`px-[10px] py-[5px] font-mono text-[11px] cursor-pointer transition-colors${borderLeft ? " border-l border-line-2" : ""} ${
        active
          ? "bg-gold text-[#1c1c1f] font-semibold"
          : "bg-board text-ink-2 hover:text-ink"
      }`}
    >
      {label}
    </button>
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
      className={`w-[90px] font-mono ${inputClass}`}
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

  const debouncedSet = useDebouncedSetter(set);

  const jitter = settings ? parseJitterMode(settings.jitter_mode) : { base: "auto" as JitterModeString, fixedMs: 40 };

  if (isLoading || !settings) return null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[420px] max-w-[420px] bg-surface border-line rounded-[3px] gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-elev-1 border-b border-line rounded-t-[3px] flex-row items-center justify-between">
          <DialogTitle className="font-mono text-[9.5px] tracking-[0.5px] text-ink-3 font-semibold uppercase">
            Configurações
          </DialogTitle>
          {savedVisible && (
            <span className="font-mono text-[9.5px] text-gold tracking-wide">
              salvo
            </span>
          )}
        </DialogHeader>

        <div className="px-[11px] py-[11px] overflow-y-auto max-h-[520px]">
          <SectionLabel>Dispositivo</SectionLabel>

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

          <SectionLabel>Conexão</SectionLabel>

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
              set={debouncedSet}
            />
          </Row>

          <SectionLabel>Áudio</SectionLabel>

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
              set={debouncedSet}
            />
          </Row>

          <SectionLabel>Sistema</SectionLabel>

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

          <SectionLabel>Aparência</SectionLabel>

          <Row>
            <SettingLabel>Tema</SettingLabel>
            <div className="flex rounded-[2px] border border-line-2 overflow-hidden">
              <ThemeButton
                value="dark"
                active={theme === "dark"}
                onSelect={(v) => { setTheme(v); applyTheme(v); }}
                label="Escuro"
              />
              <ThemeButton
                value="light"
                active={theme === "light"}
                onSelect={(v) => { setTheme(v); applyTheme(v); }}
                label="Claro"
                borderLeft
              />
            </div>
          </Row>
          <SectionLabel>Sobre</SectionLabel>

          <Row>
            <SettingLabel>Versão</SettingLabel>
            <AppVersion />
          </Row>

          <Row>
            <SettingLabel>Atualizações</SettingLabel>
            {updateState.status === "available" ? (
              <div className="flex items-center gap-2">
                <span className="font-mono text-[10px] text-gold">
                  v{updateState.version}
                </span>
                <button
                  type="button"
                  onClick={updateState.onInstall}
                  className="font-mono text-[11px] text-[#1c1c1f] bg-gold border border-gold rounded-[2px] px-3 py-[5px] cursor-pointer hover:opacity-90"
                >
                  instalar
                </button>
              </div>
            ) : (
              <button
                type="button"
                onClick={checkForUpdates}
                disabled={updateState.status === "checking" || updateState.status === "installing"}
                className="font-mono text-[11px] text-ink-2 bg-elev-2 border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:text-ink hover:border-line disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {updateState.status === "checking"
                  ? "verificando…"
                  : updateState.status === "installing"
                  ? "instalando…"
                  : "buscar atualizações"}
              </button>
            )}
          </Row>
        </div>

        <div className="flex items-center justify-between px-[13px] py-[9px] border-t border-line">
          <button
            type="button"
            onClick={handleReset}
            className="font-mono text-[11px] text-ink-3 bg-transparent border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:text-gold hover:border-gold"
          >
            Restaurar padrões
          </button>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            className="font-mono text-[11px] text-ink-2 bg-elev-2 border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:text-ink hover:border-line"
          >
            fechar
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
