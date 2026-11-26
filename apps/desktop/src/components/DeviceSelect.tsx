// Picks an audio device for one app (`appDevice` actions and the Sound screen's app
// rows), plus the id-to-name lookup action summaries use.
import type { Action, AudioDeviceInfo } from "@omni/protocol";
import { usesAppDevice } from "../lib/actions";
import { api } from "../lib/backend";
import { useResource } from "../lib/hooks";
import { Select } from "./ui";

export function DeviceSelect({
  devices,
  value,
  onChange,
  ariaLabel,
  withDefault = false,
  className,
}: {
  devices: readonly AudioDeviceInfo[];
  /** Device id; null is the system default. */
  value: string | null;
  onChange: (device: string | null) => void;
  ariaLabel: string;
  /** Offer "System default" as a choice of its own (it follows the default from then on). */
  withDefault?: boolean;
  className?: string;
}) {
  const options = devices.map((d) => ({ value: d.id, label: `${d.name}${d.isDefault ? " (default)" : ""}` }));
  return (
    <Select
      ariaLabel={ariaLabel}
      className={className}
      value={value ?? ""}
      // A device that is not connected right now still shows what was chosen.
      placeholder={value ?? "System default"}
      options={withDefault ? [{ value: "", label: "System default" }, ...options] : options}
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
