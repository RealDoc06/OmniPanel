// Picks an audio device for one app (`appDevice` actions and the Sound screen's app
// rows), plus the id-to-name lookup action summaries use.
import type { Action, AppRoute, AudioDeviceInfo } from "@omni/protocol";
import { usesAppDevice } from "../lib/actions";
import { api } from "../lib/backend";
import { useResource } from "../lib/hooks";
import { Select } from "./ui";

/** Select value that is no option: shows the placeholder. Device ids are never empty-looking. */
const NO_OPTION = "\u0000";

export function DeviceSelect({
  devices,
  value,
  onChange,
  ariaLabel,
  className,
}: {
  devices: readonly AudioDeviceInfo[];
  /** Where it is now: a device (connected or not), the system default, or spread over several devices. */
  value: AppRoute;
  /** A device id, or null for the system default (which it follows from then on). */
  onChange: (device: string | null) => void;
  ariaLabel: string;
  className?: string;
}) {
  const options = devices.map((d) => ({ value: d.id, label: `${d.name}${d.isDefault ? " (default)" : ""}` }));
  return (
    <Select
      ariaLabel={ariaLabel}
      className={className}
      value={value.type === "default" ? "" : value.type === "device" ? value.id : NO_OPTION}
      placeholder={value.type === "several" ? "Several devices" : "Not connected"}
      options={[{ value: "", label: "System default" }, ...options]}
      onChange={(id) => onChange(id || null)}
    />
  );
}

/** Device names by id for action summaries. Asks the mixer only when `action` chooses a device. */
export function useDeviceName(action: Action | null): (id: string) => string | undefined {
  const needed = usesAppDevice(action);
  const mixer = useResource(() => (needed ? api.getMixer() : Promise.resolve(null)), [needed]);
  const devices = mixer.data?.devices;
  return (id) => [...(devices?.outputs ?? []), ...(devices?.inputs ?? [])].find((d) => d.id === id)?.name;
}
