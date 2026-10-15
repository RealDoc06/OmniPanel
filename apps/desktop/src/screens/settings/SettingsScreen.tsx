// App settings: appearance, general, USB, AI, data folder, about.
import type { AiProvider } from "@omni/protocol";
import { useEffect, useId, useState } from "react";
import { Button, ComboInput, EnumSelect, Field, Notice, NumberField, ScreenHeader, Section, Switch, TextInput } from "../../components/ui";
import { api } from "../../lib/backend";
import { useResource } from "../../lib/hooks";
import { patchSection, patchSettings, run, useApp, useRuntime, useSettings } from "../../lib/store";
import { toast } from "../../lib/toast";
import { errorMessage } from "../../lib/util";
import { ThemeSettings } from "./Themes";

const TEXT_DELAY = 500;
const REPO = "https://github.com/RealDoc06/OmniPanel";

/** Opens a link in the system browser (the webview never navigates away). */
const openLink = (url: string) => void run(api.runAction({ type: "open", target: url }));

export function SettingsScreen() {
  const anchor = useApp((s) => s.anchor);
  useEffect(() => {
    if (anchor) document.getElementById(`settings-${anchor}`)?.scrollIntoView({ block: "start" });
  }, [anchor]);

  return (
    <>
      <ScreenHeader title="Settings" />
      <div className="flex-1 overflow-y-auto">
        <Section title="Appearance" id="settings-appearance">
          <ThemeSettings />
        </Section>
        <GeneralSettings />
        <UsbSettings />
        <AiSettings />
        <DataAndAbout />
      </div>
    </>
  );
}

function GeneralSettings() {
  const settings = useSettings();
  const machineName = useApp((s) => s.info.machineName);
  const ids = { name: useId(), port: useId() };
  return (
    <Section title="General" id="settings-general">
      <Field label="Name on decks" htmlFor={ids.name}>
        <TextInput
          id={ids.name}
          className="max-w-xs"
          placeholder={machineName}
          value={settings.hostName}
          onChange={(e) => patchSettings({ hostName: e.target.value }, TEXT_DELAY)}
        />
      </Field>
      <Field label="Launch at login">
        <Switch label="Launch at login" checked={settings.launchAtLogin} onChange={(launchAtLogin) => patchSettings({ launchAtLogin })} />
      </Field>
      <Field label="Start in the tray" hint="Don't open this window when OmniPanel starts.">
        <Switch label="Start in the tray" checked={settings.startHidden} onChange={(startHidden) => patchSettings({ startHidden })} />
      </Field>
      <Field label="Deck server port" htmlFor={ids.port} hint="Applies immediately. Connected decks need the new address.">
        <NumberField id={ids.port} className="max-w-32" value={settings.port} min={1024} max={65535} onChange={(port) => patchSettings({ port })} />
      </Field>
    </Section>
  );
}

function UsbSettings() {
  const usb = useSettings().usb;
  const status = useRuntime().usb;
  const id = useId();
  return (
    <Section title="USB" id="settings-usb">
      <Field label="Android over USB" hint="Forwards the deck server with adb reverse, then open localhost on the device.">
        <Switch label="Android over USB" checked={usb.enabled} onChange={(enabled) => patchSection("usb", { enabled })} />
      </Field>
      <Field
        label="adb path"
        htmlFor={id}
        hint={status.adbPath ? `Using ${status.adbPath}` : usb.enabled ? "adb not found. Install Android platform-tools or set its path." : undefined}
      >
        <TextInput
          id={id}
          className="max-w-md font-mono text-xs"
          placeholder="Detect automatically"
          value={usb.adbPath ?? ""}
          onChange={(e) => patchSection("usb", { adbPath: e.target.value.trim() || null }, TEXT_DELAY)}
        />
      </Field>
      {status.error && <Notice tone="error">{status.error}</Notice>}
    </Section>
  );
}

const PROVIDERS: Record<AiProvider, string> = {
  openai: "OpenAI",
  anthropic: "Anthropic",
  ollama: "Ollama (local)",
  openrouter: "OpenRouter",
  custom: "Custom, OpenAI-compatible",
};

const BASE_URLS: Record<AiProvider, string> = {
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com",
  ollama: "http://localhost:11434",
  openrouter: "https://openrouter.ai/api/v1",
  custom: "http://localhost:1234/v1",
};

const DEFAULT_MODELS: Partial<Record<AiProvider, string>> = { openai: "gpt-6-luna" };

function AiSettings() {
  const ai = useSettings().ai;
  const ids = { model: useId(), url: useId(), key: useId() };
  const [testing, setTesting] = useState(false);
  // Suggestions depend on the saved provider settings, so reload after they settle.
  const models = useResource(
    () => new Promise<string[]>((resolve, reject) => setTimeout(() => api.aiModels().then(resolve, reject), 600)),
    [ai.provider, ai.baseUrl, ai.apiKey],
  );
  const firstModel = models.data?.[0];

  useEffect(() => {
    if (!ai.model && firstModel) patchSection("ai", { model: firstModel });
  }, [ai.model, firstModel]);

  const test = async () => {
    setTesting(true);
    try {
      toast(await api.aiTest(), "success");
    } catch (error) {
      toast(`AI test failed: ${errorMessage(error)}`, "error");
    } finally {
      setTesting(false);
    }
  };

  const needsKey = ai.provider !== "ollama";
  const missingUrl = ai.provider === "custom" && !ai.baseUrl;

  return (
    <Section title="AI assistant" id="settings-ai">
      <Field label="Provider">
        <EnumSelect
          ariaLabel="Provider"
          className="max-w-xs"
          value={ai.provider}
          labels={PROVIDERS}
          onChange={(provider) => patchSection("ai", { provider, baseUrl: null, model: DEFAULT_MODELS[provider] ?? "" })}
        />
      </Field>
      <Field label="Model" htmlFor={ids.model}>
        <ComboInput
          id={ids.model}
          className="max-w-xs"
          value={ai.model}
          placeholder={firstModel ?? "Model name"}
          options={(models.data ?? []).map((m) => ({ value: m, label: m }))}
          onChange={(model) => patchSection("ai", { model: model.trim() }, TEXT_DELAY)}
        />
      </Field>
      <Field label="Base URL" htmlFor={ids.url} hint={missingUrl ? "Required for a custom endpoint." : "Leave empty for the default."}>
        <TextInput
          id={ids.url}
          className="max-w-md"
          placeholder={BASE_URLS[ai.provider]}
          value={ai.baseUrl ?? ""}
          onChange={(e) => patchSection("ai", { baseUrl: e.target.value.trim() || null }, TEXT_DELAY)}
        />
      </Field>
      {needsKey && (
        <Field label="API key" htmlFor={ids.key} hint="Stored in the OmniPanel config file on this computer.">
          <TextInput
            id={ids.key}
            type="password"
            className="max-w-md font-mono text-xs"
            placeholder={ai.provider === "custom" ? "Optional" : "Paste your key"}
            value={ai.apiKey}
            onChange={(e) => patchSection("ai", { apiKey: e.target.value.trim() }, TEXT_DELAY)}
          />
        </Field>
      )}
      <Field label="">
        <Button size="sm" icon="zap" disabled={testing || missingUrl} onClick={() => void test()}>
          {testing ? "Testing…" : "Test connection"}
        </Button>
      </Field>
    </Section>
  );
}

function DataAndAbout() {
  const info = useApp((s) => s.info);
  return (
    <>
      <Section title="Data" id="settings-data">
        <Field label="Data folder" hint="Config, imported images and logs.">
          <div className="flex items-center gap-2">
            <code data-selectable className="min-w-0 truncate font-mono text-xs text-muted" title={info.dataDir}>
              {info.dataDir}
            </code>
            <Button size="sm" icon="folder-open" onClick={() => void run(api.openDataDir())}>
              Open
            </Button>
          </div>
        </Field>
      </Section>
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
            <p className="text-muted">Free and open source under the MIT license. Icons by Lucide (ISC).</p>
            <div className="mt-1 flex gap-1.5">
              <Button size="sm" icon="external-link" onClick={() => openLink(REPO)}>
                Source code
              </Button>
              <Button size="sm" icon="external-link" onClick={() => openLink(`${REPO}/issues`)}>
                Report an issue
              </Button>
              <Button size="sm" icon="external-link" onClick={() => openLink("https://lucide.dev")}>
                Lucide
              </Button>
              <Button size="sm" variant="ghost" icon="power" onClick={() => void run(api.quit())}>
                Quit OmniPanel
              </Button>
            </div>
          </div>
        </div>
      </Section>
    </>
  );
}
