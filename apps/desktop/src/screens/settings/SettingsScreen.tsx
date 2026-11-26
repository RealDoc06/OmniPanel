// App settings: theme, general, OBS Studio, Home Assistant, AI assistant, advanced
// (network port, USB, settings folder, activity log), updates and about. Every section has a
// `settings-{anchor}` id, so `navigate("settings", anchor)` scrolls to it.
import type { AiProvider, AiSettings as AiConfig } from "@omni/protocol";
import { useEffect, useId, useState } from "react";
import { Button, ComboInput, Disclosure, EnumSelect, Field, inputClass, Notice, ScreenHeader, Section, Select, Switch, TextInput } from "../../components/ui";
import { api, updater } from "../../lib/backend";
import { confirm, type ConfirmRequest } from "../../lib/confirm";
import { useResource } from "../../lib/hooks";
import { flushSettings, navigate, patchSection, patchSettings, quit, run, useAnchor, useApp, useIsMac, useProfiles, useRuntime, useSettings } from "../../lib/store";
import { installUpdate, useUpdate } from "../../lib/updates";
import { cx, errorMessage } from "../../lib/util";
import { HaSettings, ObsSettings } from "./Connections";
import { ThemeSettings } from "./Themes";

const TEXT_DELAY = 500;
const REPO = "https://github.com/RealDoc06/OmniPanel";

/** Opens a link in the system browser (the webview never navigates away). */
const openLink = (url: string) => void run(api.runAction({ type: "open", target: url }));

/** Inline text link that opens in the system browser. */
function Link({ url, children }: { url: string; children: string }) {
  return (
    <button type="button" className="text-accent hover:underline" onClick={() => openLink(url)}>
      {children}
    </button>
  );
}

export function SettingsScreen() {
  useAnchor("settings");
  return (
    <>
      <ScreenHeader title="Settings" />
      <div className="flex-1 overflow-y-auto">
        <Section title="Theme" id="settings-theme">
          <ThemeSettings />
        </Section>
        <GeneralSettings />
        <ObsSettings PortInput={PortField} />
        <HaSettings />
        <AiSettings />
        <AdvancedSettings />
        <UpdateSettings />
        <About />
      </div>
    </>
  );
}

function GeneralSettings() {
  const settings = useSettings();
  const profiles = useProfiles();
  const machineName = useApp((s) => s.info.machineName);
  const mac = useIsMac();
  const ids = { name: useId(), layout: useId() };
  return (
    <Section title="General" id="settings-general">
      <Field label="Name on devices" htmlFor={ids.name} hint="How this computer appears on your phone or tablet.">
        <TextInput
          id={ids.name}
          className="max-w-xs"
          placeholder={machineName}
          value={settings.hostName}
          onChange={(e) => patchSettings({ hostName: e.target.value }, TEXT_DELAY)}
        />
      </Field>
      <Field label="Open at login">
        <Switch label="Open at login" checked={settings.launchAtLogin} onChange={(launchAtLogin) => patchSettings({ launchAtLogin })} />
      </Field>
      <Field label="Start hidden" hint={`OmniPanel keeps running in the ${mac ? "menu bar" : "system tray"}.`}>
        <Switch label="Start hidden" checked={settings.startHidden} onChange={(startHidden) => patchSettings({ startHidden })} />
      </Field>
      <Field label="Layout for new devices" htmlFor={ids.layout}>
        <Select
          id={ids.layout}
          className="max-w-xs"
          value={settings.defaultProfileId ?? ""}
          options={profiles.map((p) => ({ value: p.id, label: p.name }))}
          onChange={(defaultProfileId) => patchSettings({ defaultProfileId })}
        />
      </Field>
    </Section>
  );
}

const PROVIDERS: Record<AiProvider, string> = {
  openai: "OpenAI",
  anthropic: "Anthropic",
  ollama: "Ollama (runs on this computer)",
  openrouter: "OpenRouter",
  custom: "Other (OpenAI-compatible)",
};

const BASE_URLS: Record<AiProvider, string> = {
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com",
  ollama: "http://localhost:11434",
  openrouter: "https://openrouter.ai/api/v1",
  custom: "http://localhost:1234/v1",
};

/** Where to create an API key, per service. */
const KEY_PAGES: Partial<Record<AiProvider, string>> = {
  openai: "https://platform.openai.com/api-keys",
  anthropic: "https://console.anthropic.com/settings/keys",
  openrouter: "https://openrouter.ai/keys",
};

const DEFAULT_MODELS: Partial<Record<AiProvider, string>> = { openai: "gpt-6-luna" };

function AiSettings() {
  const ai = useSettings().ai;
  const ids = { model: useId(), url: useId(), key: useId() };
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  // Model requests must use confirmed settings, never a timed guess about persistence.
  const models = useResource(async () => {
    await flushSettings();
    const confirmed = (await api.getState()).config.settings.ai;
    if (confirmed.provider !== ai.provider || confirmed.baseUrl !== ai.baseUrl || confirmed.apiKey !== ai.apiKey) {
      return { provider: confirmed.provider, list: [] };
    }
    const list = await api.aiModels();
    const current = (await api.getState()).config.settings.ai;
    return { provider: confirmed.provider, list: current.provider === confirmed.provider && current.baseUrl === confirmed.baseUrl && current.apiKey === confirmed.apiKey ? list : [] };
  }, [ai.provider, ai.baseUrl, ai.apiKey]);
  const list = models.data?.provider === ai.provider ? models.data.list : [];
  const firstModel = models.loading ? undefined : list[0];

  useEffect(() => {
    if (!models.loading && models.data?.provider === ai.provider && !ai.model && firstModel) {
      patchSection("ai", { model: firstModel });
    }
  }, [models.loading, models.data, ai.provider, ai.model, firstModel]);

  /** Saves an AI setting; the last test result no longer applies. */
  const edit = (patch: Partial<AiConfig>, delay = 0) => {
    setResult(null);
    patchSection("ai", patch, delay);
  };

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      setResult({ ok: true, text: await flushSettings().then(() => api.aiTest()) });
    } catch (error) {
      setResult({ ok: false, text: errorMessage(error) });
    } finally {
      setTesting(false);
    }
  };

  const keyPage = KEY_PAGES[ai.provider];
  const missingUrl = ai.provider === "custom" && !ai.baseUrl;

  return (
    <Section title="AI assistant" id="settings-ai">
      <p className="mb-2">Describe a button in plain words and AI builds it.</p>
      <Field label="Service">
        <EnumSelect
          ariaLabel="Service"
          className="max-w-xs"
          value={ai.provider}
          labels={PROVIDERS}
          onChange={(provider) => edit({ provider, baseUrl: null, model: DEFAULT_MODELS[provider] ?? "" })}
        />
      </Field>
      {ai.provider === "ollama" ? (
        <Field label="">
          <p>
            Ollama runs on this computer. Get it at <Link url="https://ollama.com">ollama.com</Link>.
          </p>
        </Field>
      ) : (
        <Field label="API key" htmlFor={ids.key} hint="Saved only on this computer.">
          <div className="flex items-center gap-2">
            <TextInput
              id={ids.key}
              type="password"
              className="max-w-md font-mono text-xs"
              placeholder={ai.provider === "custom" ? "Optional" : "Paste your key"}
              value={ai.apiKey}
              onChange={(e) => edit({ apiKey: e.target.value.trim() }, TEXT_DELAY)}
            />
            {keyPage && (
              <Button size="sm" variant="ghost" icon="external-link" onClick={() => openLink(keyPage)}>
                Get a key
              </Button>
            )}
          </div>
        </Field>
      )}
      <Field label="">
        <Button size="sm" icon="zap" disabled={testing || missingUrl} onClick={() => void test()}>
          {testing ? "Testing…" : "Test connection"}
        </Button>
        {result && (
          <p role="status" data-selectable className={result.ok ? "mt-1 text-success" : "mt-1 text-danger"}>
            {result.text}
          </p>
        )}
      </Field>
      {/* Remounts per service; open for Other, which needs a server address. */}
      <Disclosure key={ai.provider} label="Advanced" defaultOpen={ai.provider === "custom"}>
        <Field label="Model" htmlFor={ids.model}>
          <ComboInput
            id={ids.model}
            className="max-w-xs"
            value={ai.model}
            placeholder={firstModel ?? "Model name"}
            options={list.map((m) => ({ value: m, label: m }))}
            onChange={(model) => edit({ model: model.trim() }, TEXT_DELAY)}
          />
          {models.error && (
            <div className="mt-1 flex items-center gap-2 text-danger">
              <span data-selectable className="min-w-0">
                Couldn't load models: {models.error}
              </span>
              <Button size="sm" onClick={models.reload}>
                Retry
              </Button>
            </div>
          )}
        </Field>
        <Field label="Server address" htmlFor={ids.url} hint={missingUrl ? "Required for Other." : "Leave empty for the default."}>
          <TextInput
            id={ids.url}
            className="max-w-md"
            placeholder={BASE_URLS[ai.provider]}
            value={ai.baseUrl ?? ""}
            onChange={(e) => edit({ baseUrl: e.target.value.trim() || null }, TEXT_DELAY)}
          />
        </Field>
      </Disclosure>
    </Section>
  );
}

/**
 * Port input: accepts only digits on save, commits on blur or Enter. Input outside `min`..`max` shows an
 * error and is never saved (no clamping). With `confirmChange`, a valid change asks
 * first and cancelling puts the saved port back.
 */
function PortField({
  value,
  onChange,
  min = 1,
  max = 65535,
  id,
  confirmChange,
}: {
  value: number;
  onChange: (port: number) => void;
  min?: number;
  max?: number;
  id?: string;
  confirmChange?: ConfirmRequest;
}) {
  const [draft, setDraft] = useState(String(value));
  const [invalid, setInvalid] = useState(false);
  const errorId = useId();
  useEffect(() => {
    setDraft(String(value));
    setInvalid(false);
  }, [value]);

  const commit = async () => {
    const port = Number(draft);
    if (!/^\d+$/.test(draft) || !Number.isInteger(port) || port < min || port > max) return setInvalid(true);
    setInvalid(false);
    if (port === value) return setDraft(String(value));
    if (confirmChange && !(await confirm(confirmChange))) return setDraft(String(value));
    onChange(port);
  };

  return (
    <div>
      <input
        id={id}
        type="text"
        inputMode="numeric"
        pattern="[0-9]*"
        autoComplete="off"
        aria-invalid={invalid}
        aria-describedby={invalid ? errorId : undefined}
        className={cx(inputClass, "max-w-32 tabular-nums", invalid && "border-danger")}
        value={draft}
        onChange={(e) => {
          setDraft(e.target.value);
          setInvalid(false);
        }}
        onBlur={() => void commit()}
        // Enter commits through blur, so one edit never asks twice.
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          e.preventDefault();
          e.currentTarget.blur();
        }}
      />
      {invalid && <p id={errorId} role="alert" className="mt-1 text-xs text-danger">Use a number from {min} to {max}.</p>}
    </div>
  );
}

const PORT_CONFIRM = {
  title: "Change the network port?",
  body: "Devices connected now will disconnect. Each one must be connected again with a new code.",
  confirmLabel: "Change port",
};

function AdvancedSettings() {
  const settings = useSettings();
  const status = useRuntime().usb;
  const dataDir = useApp((s) => s.info.dataDir);
  const ids = { port: useId(), adb: useId() };
  const { usb } = settings;
  return (
    <Section title="Advanced" id="settings-advanced">
      <Field label="Network port" htmlFor={ids.port} hint={`Change only if another app uses ${settings.port}. Ports below 1024 need admin rights.`}>
        <PortField id={ids.port} value={settings.port} min={1} max={65535} confirmChange={PORT_CONFIRM} onChange={(port) => patchSettings({ port })} />
      </Field>
      <div id="settings-usb">
        <Field label="USB connection (Android)" hint="Lets Android devices connect with a cable.">
          <Switch label="USB connection (Android)" checked={usb.enabled} onChange={(enabled) => patchSection("usb", { enabled })} />
        </Field>
      </div>
      <Field
        label="Platform-tools (adb) location"
        htmlFor={ids.adb}
        hint={status.adbPath ? `Using ${status.adbPath}` : "Not found. Install Android platform-tools or enter where adb is."}
      >
        <TextInput
          id={ids.adb}
          className="max-w-md font-mono text-xs"
          placeholder="Found automatically"
          value={usb.adbPath ?? ""}
          onChange={(e) => patchSection("usb", { adbPath: e.target.value.trim() || null }, TEXT_DELAY)}
        />
      </Field>
      {status.error && <Notice tone="error">{status.error}</Notice>}
      <Field label="Settings folder" hint="Your layouts, images and logs.">
        <div className="flex items-center gap-2">
          <code data-selectable className="min-w-0 truncate font-mono text-xs text-muted" title={dataDir}>
            {dataDir}
          </code>
          <Button size="sm" icon="folder-open" onClick={() => void run(api.openDataDir())}>
            Open folder
          </Button>
        </div>
      </Field>
      <Field label="Activity log">
        <Button size="sm" onClick={() => navigate("log")}>
          Open
        </Button>
      </Field>
    </Section>
  );
}

/** What is installed, or the newer version with its one next step: install it, or get the package by hand. */
function UpdateSettings() {
  const settings = useSettings();
  const status = useUpdate((s) => s.status);
  const current = useApp((s) => s.info.version);
  // From the click until the shell reports progress, and between its steps.
  const [starting, setStarting] = useState(false);
  const install = async () => {
    setStarting(true);
    await run(installUpdate());
    setStarting(false);
  };
  const busy = starting || status.state === "downloading" || status.state === "installing";
  // deb and rpm installs, which the package manager owns.
  const manual = status.state === "available" && !status.canInstall;
  return (
    <Section title="Updates" id="settings-updates">
      {"version" in status ? (
        <Field label="New version" hint={manual ? "This copy can't update itself. Download the new installer and open it." : undefined}>
          <div className="flex items-center gap-2">
            <span data-selectable>OmniPanel {status.version}</span>
            {manual ? (
              <Button size="sm" icon="external-link" onClick={() => openLink(`${REPO}/releases/tag/v${status.version}`)}>
                Download
              </Button>
            ) : (
              <Button size="sm" variant="primary" icon="download" disabled={busy} onClick={() => void install()}>
                Install and restart
              </Button>
            )}
            <span role="status" className="text-muted">
              {status.state === "downloading" && `Downloading${status.percent === null ? "…" : ` ${status.percent}%`}`}
              {status.state === "installing" && "Installing…"}
            </span>
          </div>
        </Field>
      ) : (
        <Field label="This version">
          <div className="flex items-center gap-2">
            <span role="status" data-selectable>
              {status.state === "checking" ? "Checking…" : `OmniPanel ${current}${status.state === "upToDate" ? " is the newest version." : ""}`}
            </span>
            <Button size="sm" icon="refresh-cw" disabled={status.state === "checking"} onClick={() => void run(updater.check())}>
              Check now
            </Button>
          </div>
        </Field>
      )}
      <Field label="Check automatically" hint="Once a day. Nothing is installed until you say so.">
        <Switch label="Check automatically" checked={settings.checkForUpdates} onChange={(checkForUpdates) => patchSettings({ checkForUpdates })} />
      </Field>
    </Section>
  );
}

function About() {
  const info = useApp((s) => s.info);
  return (
    <Section title="About" id="settings-about">
      <div className="flex items-start gap-4">
        <img src="/logo.svg" alt="" className="size-12" draggable={false} />
        <div className="flex flex-col gap-1">
          <p>
            <span className="font-semibold">OmniPanel {info.version}</span>
            <span className="text-muted">
              {" "}
              · {info.os} {info.arch}
            </span>
          </p>
          <p className="text-muted">
            Free and open source under the MIT license. Icons by <Link url="https://lucide.dev">Lucide</Link> (ISC).
          </p>
          <div className="mt-1 flex gap-1.5">
            <Button size="sm" icon="external-link" onClick={() => openLink(REPO)}>
              Source code
            </Button>
            <Button size="sm" icon="external-link" onClick={() => openLink(`${REPO}/issues`)}>
              Report an issue
            </Button>
            <Button size="sm" variant="ghost" icon="power" onClick={() => void run(quit())}>
              Quit OmniPanel
            </Button>
          </div>
        </div>
      </div>
    </Section>
  );
}
