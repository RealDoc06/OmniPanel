// OBS (obs-websocket v5) and Home Assistant connection settings with live status.
import type { ConnState, ConnectionStatus } from "@omni/protocol";
import { useId } from "react";
import { Field, NumberField, ScreenHeader, Section, StatusDot, Switch, TextInput } from "../../components/ui";
import { patchSection, useRuntime, useSettings } from "../../lib/store";

const TEXT_DELAY = 500;

const STATE_LABELS: Record<ConnState, string> = { disabled: "Off", connecting: "Connecting…", connected: "Connected", error: "Error" };
const STATE_TONES = { disabled: "off", connecting: "warn", connected: "ok", error: "error" } as const satisfies Record<ConnState, string>;

function Status({ status }: { status: ConnectionStatus }) {
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-xs">
      <StatusDot tone={STATE_TONES[status.state]} />
      <span className={status.state === "error" ? "text-danger" : "text-muted"}>{STATE_LABELS[status.state]}</span>
      {status.message && <span className="truncate text-muted" title={status.message}>· {status.message}</span>}
    </span>
  );
}

export function IntegrationsScreen() {
  const { obs, homeAssistant: ha } = useSettings();
  const runtime = useRuntime();
  const ids = { obsHost: useId(), obsPort: useId(), obsPassword: useId(), haUrl: useId(), haToken: useId() };

  return (
    <>
      <ScreenHeader title="Integrations" />
      <div className="flex-1 overflow-y-auto">
        <Section
          title="OBS Studio"
          actions={
            <>
              <Status status={runtime.obs} />
              <Switch label="Enable OBS" checked={obs.enabled} onChange={(enabled) => patchSection("obs", { enabled })} />
            </>
          }
        >
          <p className="mb-2 text-muted">
            In OBS open Tools &gt; WebSocket Server Settings, enable the server and copy the port and password here.
          </p>
          <Field label="Host" htmlFor={ids.obsHost}>
            <TextInput id={ids.obsHost} className="max-w-sm" value={obs.host} onChange={(e) => patchSection("obs", { host: e.target.value.trim() }, TEXT_DELAY)} />
          </Field>
          <Field label="Port" htmlFor={ids.obsPort}>
            <NumberField id={ids.obsPort} className="max-w-32" value={obs.port} min={1} max={65535} onChange={(port) => patchSection("obs", { port })} />
          </Field>
          <Field label="Password" htmlFor={ids.obsPassword} hint="Leave empty if authentication is off.">
            <TextInput
              id={ids.obsPassword}
              type="password"
              className="max-w-sm"
              value={obs.password}
              onChange={(e) => patchSection("obs", { password: e.target.value }, TEXT_DELAY)}
            />
          </Field>
        </Section>

        <Section
          title="Home Assistant"
          actions={
            <>
              <Status status={runtime.homeAssistant} />
              <Switch label="Enable Home Assistant" checked={ha.enabled} onChange={(enabled) => patchSection("homeAssistant", { enabled })} />
            </>
          }
        >
          <p className="mb-2 text-muted">
            Create a long-lived access token in Home Assistant under your profile &gt; Security.
          </p>
          <Field label="URL" htmlFor={ids.haUrl}>
            <TextInput
              id={ids.haUrl}
              className="max-w-sm"
              placeholder="http://homeassistant.local:8123"
              value={ha.url}
              onChange={(e) => patchSection("homeAssistant", { url: e.target.value.trim() }, TEXT_DELAY)}
            />
          </Field>
          <Field label="Access token" htmlFor={ids.haToken}>
            <TextInput
              id={ids.haToken}
              type="password"
              className="max-w-sm"
              value={ha.token}
              onChange={(e) => patchSection("homeAssistant", { token: e.target.value.trim() }, TEXT_DELAY)}
            />
          </Field>
        </Section>
      </div>
    </>
  );
}
