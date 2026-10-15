// Picks a `VolumeTarget`: system output, mic, an app (from the mixer) or an OBS input.
import type { VolumeTarget } from "@omni/protocol";
import { VOLUME_TARGET_LABELS } from "../lib/actions";
import { api } from "../lib/backend";
import { useResource } from "../lib/hooks";
import { ComboInput, EnumSelect } from "./ui";

export function VolumeTargetPicker({ value, onChange }: { value: VolumeTarget; onChange: (target: VolumeTarget) => void }) {
  return (
    <div className="flex flex-col gap-1.5">
      <EnumSelect
        ariaLabel="Target"
        value={value.type}
        labels={VOLUME_TARGET_LABELS}
        onChange={(type) => {
          switch (type) {
            case "master":
            case "mic":
              return onChange({ type });
            case "app":
              return onChange({ type, app: value.type === "app" ? value.app : "" });
            case "obsInput":
              return onChange({ type, input: value.type === "obsInput" ? value.input : "" });
          }
        }}
      />
      {value.type === "app" && <AppField value={value.app} onChange={(app) => onChange({ type: "app", app })} />}
      {value.type === "obsInput" && <ObsInputField value={value.input} onChange={(input) => onChange({ type: "obsInput", input })} />}
    </div>
  );
}

function AppField({ value, onChange }: { value: string; onChange: (app: string) => void }) {
  const mixer = useResource(() => api.getMixer());
  const options = (mixer.data?.apps ?? []).map((a) => ({ value: a.id, label: a.name }));
  return (
    <>
      <ComboInput ariaLabel="Application" value={value} onChange={onChange} options={options} placeholder="App name or id, e.g. Spotify" />
      {mixer.data && options.length === 0 && <p className="text-xs text-muted">No app is playing audio right now. Type its name.</p>}
    </>
  );
}

/** OBS input name, with suggestions while OBS is connected. */
export function ObsInputField({ value, onChange }: { value: string; onChange: (input: string) => void }) {
  const inventory = useResource(() => api.obsInventory());
  return (
    <>
      <ComboInput
        ariaLabel="OBS input"
        value={value}
        onChange={onChange}
        options={(inventory.data?.inputs ?? []).map((i) => ({ value: i, label: i }))}
        placeholder="Input name, e.g. Mic/Aux"
      />
      {inventory.error && <p className="text-xs text-muted">OBS is offline. Type the input name.</p>}
    </>
  );
}
