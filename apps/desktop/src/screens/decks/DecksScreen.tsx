// Devices. First run (no devices): the connect panel fills the screen. Otherwise one row per
// paired device (connection, name, the layout it shows); clicking a row opens its options,
// Edit buttons and Remove. While the connect panel is closed, a quiet line under the list
// offers to connect another.
import type { DeckSession, Device } from "@omni/protocol";
import { useEffect, useId, useState, type MouseEvent } from "react";
import { Button, IconButton, Notice, Range, ScreenHeader, Select, StatusDot, Switch, TextInput } from "../../components/ui";
import { confirm } from "../../lib/confirm";
import { useNow } from "../../lib/hooks";
import { navigate, saveDevice, unpairDevice, useAnchor, useApp, useProfiles, useRuntime } from "../../lib/store";
import { cx, relativeTime } from "../../lib/util";
import { createProfile, openProfile } from "../profiles/editor";
import { ConnectPanel } from "./ConnectPanel";

/** "Shows" option that creates a layout for the device. Ids are alphanumeric, so it can't clash. */
const NEW_LAYOUT = "__new";

export function DecksScreen() {
  const devices = useApp((s) => s.config.devices);
  const runtime = useRuntime();
  const lastPairedId = useApp((s) => s.lastPaired?.id);
  const anchor = useApp((s) => s.anchor);
  const [connecting, setConnecting] = useState(anchor === "connect");
  const [openId, setOpenId] = useState<string | null>(null);
  useAnchor("decks");
  const firstRun = devices.length === 0;

  // "Connected. It shows …" is only for the device that connected while Devices was open.
  useEffect(() => () => useApp.setState({ lastPaired: null }), []);

  return (
    <>
      <ScreenHeader title="Devices" />
      <div className="flex flex-1 flex-col overflow-y-auto">
        {runtime.server.error && (
          <div className="px-7 pt-4">
            <Notice tone="error">Devices can't connect: {runtime.server.error}</Notice>
          </div>
        )}
        {(firstRun || connecting) && <ConnectPanel hero={firstRun} onClose={firstRun ? undefined : () => setConnecting(false)} />}
        {devices.length > 0 && (
          <ul>
            {devices.map((device) => (
              <DeviceRow
                key={device.id}
                device={device}
                session={runtime.sessions.find((s) => s.deviceId === device.id)}
                justConnected={device.id === lastPairedId}
                open={openId === device.id}
                onToggle={() => setOpenId((id) => (id === device.id ? null : device.id))}
              />
            ))}
          </ul>
        )}
        {!firstRun && !connecting && (
          <p className="px-7 py-4 text-muted">
            Any phone, tablet or browser on this Wi-Fi can be a deck.{" "}
            <button type="button" className="text-fg hover:underline" onClick={() => setConnecting(true)}>
              Connect another device
            </button>
          </p>
        )}
      </div>
    </>
  );
}

const isInteractive = (target: EventTarget) => target instanceof Element && target.closest("input, select, button, label, a") !== null;

function DeviceRow({
  device,
  session,
  justConnected,
  open,
  onToggle,
}: {
  device: Device;
  session: DeckSession | undefined;
  justConnected: boolean;
  open: boolean;
  onToggle: () => void;
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
    ? `Connected ${session.transport === "usb" ? "by USB" : "over Wi-Fi"}${device.platform ? ` · ${device.platform}` : ""}`
    : `Not connected · last seen ${relativeTime(device.lastSeen, now)}`;

  // The whole row toggles the options, except its own controls.
  const onRowClick = (e: MouseEvent<HTMLDivElement>) => {
    if (!isInteractive(e.target)) onToggle();
  };

  return (
    <li className="border-b border-line">
      <div className={cx("flex min-h-14 flex-wrap items-center gap-x-4 gap-y-1 py-2 pr-4 pl-7", !open && "hover:bg-surface")} onClick={onRowClick}>
        <StatusDot tone={session ? "ok" : "off"} label={session ? "Connected" : "Not connected"} />
        <div className="flex min-w-0 flex-1 flex-col">
          <TextInput
            aria-label="Device name"
            value={device.name}
            onChange={(e) => update({ name: e.target.value }, 500)}
            onBlur={(e) => !e.target.value.trim() && update({ name: "Device" })}
            className="-ml-1.5 h-6 w-48 border-transparent px-1.5 text-[14px] font-semibold hover:border-line focus:border-line"
          />
          <span title={session ? session.address : device.platform} className="truncate text-xs text-muted">
            {status}
          </span>
        </div>
        <label className="flex items-center gap-2 text-muted">
          Shows
          <Select
            ariaLabel="Shows"
            className="w-52"
            value={own?.id ?? ""}
            options={[
              { value: "", label: fallback ? `${fallback.name} (for new devices)` : "Default" },
              ...profiles.map((p) => ({ value: p.id, label: p.name })),
              { value: NEW_LAYOUT, label: "New layout…" },
            ]}
            onChange={onShows}
          />
        </label>
        <IconButton
          icon="chevron-down"
          label={open ? "Hide options" : "Show options"}
          aria-expanded={open}
          aria-controls={optionsId}
          className={cx(open && "rotate-180")}
          onClick={onToggle}
        />
      </div>
      {justConnected && shown && <p className="px-7 pb-3 text-success">Connected. It shows the {shown.name} layout.</p>}
      <div id={optionsId} hidden={!open} className="pr-4 pb-4 pl-[52px]">
        <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
          <Toggle label="Vibrate on press" checked={device.deck.haptics} onChange={(haptics) => deck({ haptics })} />
          <Toggle label="Keep screen on" checked={device.deck.keepAwake} onChange={(keepAwake) => deck({ keepAwake })} />
          <Toggle label="Show top bar" checked={device.deck.showStatusBar} onChange={(showStatusBar) => deck({ showStatusBar })} />
          <Toggle label="Rotate to fit screen" checked={device.deck.autoRotate} onChange={(autoRotate) => deck({ autoRotate })} />
          <label className="flex items-center gap-2">
            Dim screen
            <Range
              label="Dim screen"
              className="w-40"
              min={0}
              max={0.8}
              step={0.05}
              value={device.deck.dim}
              format={(v) => (v === 0 ? "Off" : `${Math.round(v * 100)}%`)}
              onChange={(dim) => deck({ dim }, 250)}
            />
          </label>
        </div>
        <div className="mt-3 flex gap-2">
          <Button size="sm" icon="layout-grid" disabled={!shown} onClick={editButtons}>
            Edit buttons
          </Button>
          <Button size="sm" variant="danger" onClick={() => void remove()}>
            Remove device
          </Button>
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
