// Settings sections for OBS Studio (obs-websocket v5) and Home Assistant with live
// status. The parent supplies the same validated port input used for the network port.
import type { ConnState, ConnectionStatus } from "@omni/protocol";
import { useId, type ComponentType } from "react";
import { Disclosure, Field, Notice, Section, StatusDot, Switch, TextInput } from "../../components/ui";
import { patchSection, useRuntime, useSettings } from "../../lib/store";

const TEXT_DELAY = 500;

const STATE_LABELS: Record<ConnState, string> = { disabled: "Off", connecting: "Connecting…", connected: "Connected", error: "Can't connect" };
const STATE_TONES = { disabled: "off", connecting: "warn", connected: "ok", error: "error" } as const satisfies Record<ConnState, string>;

/** Header status word; the host's message (versions, error) is in the tooltip. */
function Status({ status }: { status: ConnectionStatus }) {
  return (
    <span className="flex items-center gap-1.5 text-xs" title={status.message ?? undefined}>
      <StatusDot tone={STATE_TONES[status.state]} />
      <span className={status.state === "error" ? "text-danger" : "text-muted"}>{STATE_LABELS[status.state]}</span>
    </span>
  );
}

function ConnectError({ status }: { status: ConnectionStatus }) {
  if (status.state !== "error" || !status.message) return null;
  return (
    <div className="mb-2">
      <Notice tone="error">{status.message}</Notice>
    </div>
  );
}

export function ObsSettings({ PortInput }: {
  PortInput: ComponentType<{ id: string; value: number; onChange: (port: number) => void }>;
}) {
  const obs = useSettings().obs;
  const status = useRuntime().obs;
  const ids = { password: useId(), host: useId(), port: useId() };
  return (
    <Section
      title="OBS Studio"
      id="settings-obs"
      actions={
        <>
          <Status status={status} />
          <Switch label="Connect to OBS" checked={obs.enabled} onChange={(enabled) => patchSection("obs", { enabled })} />
        </>
      }
    >
      <ConnectError status={status} />
      <p className="mb-2">
        In OBS, open Tools &gt; WebSocket Server Settings, turn on Enable WebSocket server, click Show Connect Info and copy the password here.
      </p>
      <Field label="Password" htmlFor={ids.password} hint="Leave empty if OBS doesn't show one.">
        <TextInput
          id={ids.password}
          type="password"
          className="max-w-sm"
          value={obs.password}
          onChange={(e) => patchSection("obs", { password: e.target.value }, TEXT_DELAY)}
        />
      </Field>
      <Disclosure label="Advanced">
        <Field label="Computer running OBS" htmlFor={ids.host} hint="127.0.0.1 is this computer.">
          <TextInput id={ids.host} className="max-w-sm" value={obs.host} onChange={(e) => patchSection("obs", { host: e.target.value.trim() }, TEXT_DELAY)} />
        </Field>
        <Field label="Port" htmlFor={ids.port}>
          <PortInput id={ids.port} value={obs.port} onChange={(port) => patchSection("obs", { port })} />
        </Field>
      </Disclosure>
    </Section>
  );
}

export function HaSettings() {
  const ha = useSettings().homeAssistant;
  const status = useRuntime().homeAssistant;
  const ids = { url: useId(), token: useId() };
  return (
    <Section
      title="Home Assistant"
      id="settings-ha"
      actions={
        <>
          <Status status={status} />
          <Switch label="Connect to Home Assistant" checked={ha.enabled} onChange={(enabled) => patchSection("homeAssistant", { enabled })} />
        </>
      }
    >
      <ConnectError status={status} />
      <Field label="Address" htmlFor={ids.url}>
        <TextInput
          id={ids.url}
          className="max-w-sm"
          placeholder="http://homeassistant.local:8123"
          value={ha.url}
          onChange={(e) => patchSection("homeAssistant", { url: e.target.value.trim() }, TEXT_DELAY)}
        />
      </Field>
      <Field label="Access token" htmlFor={ids.token} hint="In Home Assistant: your profile > Security > Long-lived access tokens > Create token.">
        <TextInput
          id={ids.token}
          type="password"
          className="max-w-sm"
          value={ha.token}
          onChange={(e) => patchSection("homeAssistant", { token: e.target.value.trim() }, TEXT_DELAY)}
        />
      </Field>
    </Section>
  );
}
