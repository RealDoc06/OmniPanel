// Devices: the connect panel (open by default while there are none), then one row per
// paired device with its connection, the layout it shows, Edit buttons, Options and Remove.
import type { DeckSession, Device } from "@omni/protocol";
import { useEffect, useId, useState } from "react";
import { Button, EmptyState, Notice, Range, ScreenHeader, Select, StatusDot, Switch, TextInput } from "../../components/ui";
import { confirm } from "../../lib/confirm";
import { useNow } from "../../lib/hooks";
import { navigate, saveDevice, unpairDevice, useApp, useProfiles, useRuntime } from "../../lib/store";
import { relativeTime } from "../../lib/util";
import { createProfile, openProfile } from "../profiles/editor";
import { ConnectPanel } from "./ConnectPanel";

/** "Shows" option that creates a layout for the device. Ids are alphanumeric, so it can't clash. */
const NEW_LAYOUT = "__new";

export function DecksScreen() {
  const devices = useApp((s) => s.config.devices);
  const runtime = useRuntime();
  const lastPairedId = useApp((s) => s.lastPaired?.id);
  const [connecting, setConnecting] = useState(devices.length === 0);
  const [optionsFor, setOptionsFor] = useState<string | null>(null);

  // "Connected. It shows …" is only for the device that connected while Devices was open.
  useEffect(() => () => useApp.setState({ lastPaired: null }), []);

  const connect = (
    <Button variant="primary" icon="plus" onClick={() => setConnecting(true)}>
      Connect a device
    </Button>
  );
  return (
    <>
      <ScreenHeader title="Devices">{!connecting && connect}</ScreenHeader>
      <div className="flex-1 overflow-y-auto">
        {runtime.server.error && (
          <div className="px-6 pt-4">
            <Notice tone="error">Devices can't connect: {runtime.server.error}</Notice>
          </div>
        )}
        {connecting && <ConnectPanel onClose={() => setConnecting(false)} />}
        {devices.length > 0 ? (
          <ul>
            {devices.map((device) => (
              <DeviceRow
                key={device.id}
                device={device}
                session={runtime.sessions.find((s) => s.deviceId === device.id)}
                justConnected={device.id === lastPairedId}
                optionsOpen={optionsFor === device.id}
                onToggleOptions={() => setOptionsFor((id) => (id === device.id ? null : device.id))}
              />
            ))}
          </ul>
        ) : (
          !connecting && (
            <EmptyState icon="tablet-smartphone" title="No devices yet. Any phone, tablet or browser on the same Wi-Fi can be one.">
              {connect}
            </EmptyState>
          )
        )}
      </div>
    </>
  );
}

function DeviceRow({
  device,
  session,
  justConnected,
  optionsOpen,
  onToggleOptions,
}: {
  device: Device;
  session: DeckSession | undefined;
  justConnected: boolean;
  optionsOpen: boolean;
  onToggleOptions: () => void;
}) {
  const profiles = useProfiles();
  const optionsId = useId();
  const defaultProfileId = useApp((s) => s.config.settings.defaultProfileId);
  const now = useNow(30_000);
  const update = (patch: Partial<Device>, delay = 0) => saveDevice({ ...device, ...patch }, delay);
  const deck = (patch: Partial<Device["deck"]>, delay = 0) => update({ deck: { ...device.deck, ...patch } }, delay);

  // Same fallback as the host (`Config::profile_for_device`): own layout, the default, the first.
  const own = profiles.find((p) => p.id === device.profileId);
  const fallback = profiles.find((p) => p.id === defaultProfileId) ?? profiles[0];
  const shown = own ?? fallback;

  const onShows = (value: string) => {
    if (value !== NEW_LAYOUT) return update({ profileId: value || null });
    // createProfile also opens the new layout in the editor.
    update({ profileId: createProfile(`${device.name} buttons`) });
    navigate("profiles");
  };

  const editButtons = () => {
    if (!shown) return;
    openProfile(shown.id);
    navigate("profiles");
  };

  const remove = async () => {
    const ok = await confirm({
      title: `Remove ${device.name}?`,
      body: "It will need a new code to connect again.",
      confirmLabel: "Remove",
      danger: true,
    });
    if (ok) void unpairDevice(device.id);
  };

  const status = session
    ? `Connected ${session.transport === "usb" ? "by USB cable" : "over Wi-Fi"}${device.platform ? ` · ${device.platform}` : ""}`
    : `Not connected · last seen ${relativeTime(device.lastSeen, now)}`;

  return (
    <li className="border-b border-line px-6 py-3">
      <div className="flex flex-wrap items-center gap-3">
        <StatusDot tone={session ? "ok" : "off"} label={session ? "Connected" : "Not connected"} />
        <TextInput
          aria-label="Device name"
          value={device.name}
          onChange={(e) => update({ name: e.target.value }, 500)}
          onBlur={(e) => !e.target.value.trim() && update({ name: "Device" })}
          className="h-7 w-36 shrink-0 border-transparent bg-transparent px-1 text-[14px] font-semibold hover:border-line focus:border-line"
        />
        <span title={session ? session.address : device.platform} className="min-w-56 flex-1 text-muted">
          {status}
        </span>
        <label className="flex max-w-full items-center gap-3">
          <span className="shrink-0">Shows</span>
          <Select
            ariaLabel="Shows"
            className="h-7 w-64 min-w-0"
            value={own?.id ?? ""}
            options={[
              { value: "", label: `Same as new devices (${fallback?.name ?? "none"})` },
              ...profiles.map((p) => ({ value: p.id, label: p.name })),
              { value: NEW_LAYOUT, label: "New layout…" },
            ]}
            onChange={onShows}
          />
        </label>
        <Button size="sm" disabled={!shown} onClick={editButtons}>
          Edit buttons
        </Button>
        <Button
          size="sm"
          variant="ghost"
          icon={optionsOpen ? "chevron-down" : "chevron-right"}
          aria-expanded={optionsOpen}
          aria-controls={optionsId}
          onClick={onToggleOptions}
        >
          Options
        </Button>
        <Button size="sm" variant="ghost" className="text-danger" onClick={remove}>
          Remove
        </Button>
      </div>
      {justConnected && shown && <p className="mt-1 pl-5">Connected. It shows the {shown.name} layout.</p>}
      <div id={optionsId} hidden={!optionsOpen}>
        <div className="mt-2 flex flex-wrap items-center gap-x-6 gap-y-2 pl-5">
          <Toggle label="Vibrate on press" checked={device.deck.haptics} onChange={(haptics) => deck({ haptics })} />
          <Toggle label="Keep screen on" checked={device.deck.keepAwake} onChange={(keepAwake) => deck({ keepAwake })} />
          <Toggle label="Show top bar" checked={device.deck.showStatusBar} onChange={(showStatusBar) => deck({ showStatusBar })} />
          <Toggle label="Rotate to fit screen" checked={device.deck.autoRotate} onChange={(autoRotate) => deck({ autoRotate })} />
          <label className="flex items-center gap-2">
            Dim screen
            <Range
              label="Dim screen"
              className="w-44"
              min={0}
              max={0.8}
              step={0.05}
              value={device.deck.dim}
              format={(v) => (v === 0 ? "Off" : `${Math.round(v * 100)}%`)}
              onChange={(dim) => deck({ dim }, 250)}
            />
          </label>
        </div>
      </div>
    </li>
  );
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="flex items-center gap-2">
      <Switch label={label} checked={checked} onChange={onChange} />
      {label}
    </label>
  );
}
