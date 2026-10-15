// Paired decks: status, profile assignment, per-device deck settings, pairing.
import type { DeckSession, Device } from "@omni/protocol";
import { useState } from "react";
import { I } from "../../components/Icon";
import { Button, EmptyState, Notice, Range, ScreenHeader, Select, StatusDot, Switch, TextInput } from "../../components/ui";
import { confirm } from "../../lib/confirm";
import { useNow } from "../../lib/hooks";
import { patchSettings, saveDevice, unpairDevice, useApp, useProfiles, useRuntime } from "../../lib/store";
import { relativeTime } from "../../lib/util";
import { PairDialog } from "./PairDialog";

export function DecksScreen() {
  const devices = useApp((s) => s.config.devices);
  const runtime = useRuntime();
  const [pairing, setPairing] = useState(false);

  return (
    <>
      <ScreenHeader title="Decks">
        <DefaultProfileSelect />
        <Button variant="primary" icon="plus" onClick={() => setPairing(true)}>
          Pair a device
        </Button>
      </ScreenHeader>
      <div className="flex-1 overflow-y-auto">
        {runtime.server.error && (
          <div className="px-6 pt-4">
            <Notice tone="error">Deck server: {runtime.server.error}</Notice>
          </div>
        )}
        {devices.length === 0 ? (
          <EmptyState icon="tablet-smartphone" title="No decks yet. Any phone, tablet or browser on your network can be one.">
            <Button variant="primary" icon="plus" onClick={() => setPairing(true)}>
              Pair a device
            </Button>
          </EmptyState>
        ) : (
          <ul>
            {devices.map((device) => (
              <DeviceRow key={device.id} device={device} session={runtime.sessions.find((s) => s.deviceId === device.id)} />
            ))}
          </ul>
        )}
      </div>
      {pairing && <PairDialog onClose={() => setPairing(false)} />}
    </>
  );
}

function DefaultProfileSelect() {
  const profiles = useProfiles();
  const value = useApp((s) => s.config.settings.defaultProfileId);
  return (
    <label className="flex items-center gap-2 text-muted">
      New decks use
      <Select
        className="w-40"
        value={value ?? ""}
        options={profiles.map((p) => ({ value: p.id, label: p.name }))}
        onChange={(id) => patchSettings({ defaultProfileId: id })}
      />
    </label>
  );
}

function DeviceRow({ device, session }: { device: Device; session: DeckSession | undefined }) {
  const profiles = useProfiles();
  const defaultName = useApp((s) => s.config.profiles.find((p) => p.id === s.config.settings.defaultProfileId)?.name);
  const now = useNow(30_000);
  const update = (patch: Partial<Device>, delay = 0) => saveDevice({ ...device, ...patch }, delay);
  const deck = (patch: Partial<Device["deck"]>, delay = 0) => update({ deck: { ...device.deck, ...patch } }, delay);

  const unpair = async () => {
    const ok = await confirm({
      title: `Unpair ${device.name}?`,
      body: "The device forgets its token and has to be paired again to connect.",
      confirmLabel: "Unpair",
      danger: true,
    });
    if (ok) void unpairDevice(device.id);
  };

  return (
    <li className="border-b border-line px-6 py-3">
      <div className="flex items-center gap-3">
        <StatusDot tone={session ? "ok" : "off"} label={session ? "Online" : "Offline"} />
        <TextInput
          aria-label="Device name"
          value={device.name}
          onChange={(e) => update({ name: e.target.value }, 500)}
          onBlur={(e) => !e.target.value.trim() && update({ name: "Deck" })}
          className="h-7 w-52 border-transparent bg-transparent px-1 text-[14px] font-semibold hover:border-line focus:border-line"
        />
        <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate text-muted">
          {session ? (
            <>
              <I name={session.transport === "usb" ? "usb" : "wifi"} className="size-3.5" />
              {session.transport === "usb" ? "USB" : "Wi-Fi"} · {session.address}
            </>
          ) : (
            <>Offline · seen {relativeTime(device.lastSeen, now)}</>
          )}
          {device.platform && <> · {device.platform}</>}
        </span>
        <Select
          ariaLabel="Profile"
          className="w-56"
          value={device.profileId ?? ""}
          options={[
            { value: "", label: `Default profile (${defaultName ?? "first"})` },
            ...profiles.map((p) => ({ value: p.id, label: p.name })),
          ]}
          onChange={(id) => update({ profileId: id || null })}
        />
        <Button variant="danger" size="sm" onClick={unpair}>
          Unpair
        </Button>
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-x-6 gap-y-2 pl-5 text-muted">
        <Toggle label="Haptics" checked={device.deck.haptics} onChange={(haptics) => deck({ haptics })} />
        <Toggle label="Keep awake" checked={device.deck.keepAwake} onChange={(keepAwake) => deck({ keepAwake })} />
        <Toggle label="Status bar" checked={device.deck.showStatusBar} onChange={(showStatusBar) => deck({ showStatusBar })} />
        <Toggle label="Rotate grid to fit screen" checked={device.deck.autoRotate} onChange={(autoRotate) => deck({ autoRotate })} />
        <label className="flex items-center gap-2">
          Dim
          <Range
            label="Dim"
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
