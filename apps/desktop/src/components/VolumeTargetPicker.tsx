// Picks a `VolumeTarget`: computer sound, microphone, an app (by name, from the mixer)
// or an OBS audio source.
import type { VolumeTarget } from "@omni/protocol";
import { VOLUME_TARGET_LABELS } from "../lib/actions";
import { api } from "../lib/backend";
import { useResource } from "../lib/hooks";
import { ComboInput, EnumSelect } from "./ui";

export function VolumeTargetPicker({ value, onChange }: { value: VolumeTarget; onChange: (target: VolumeTarget) => void }) {
  return (
    <div className="flex flex-col gap-1.5">
      <EnumSelect
        ariaLabel="Sound"
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

/** App name. Suggestions are the apps the mixer knows; the host matches names too. */
function AppField({ value, onChange }: { value: string; onChange: (app: string) => void }) {
  const mixer = useResource(() => api.getMixer());
  const options = (mixer.data?.apps ?? []).map((a) => ({ value: a.name, label: a.name }));
  return (
    <>
      <ComboInput ariaLabel="App" value={value} onChange={onChange} options={options} placeholder="Pick or type an app" />
      {mixer.data && options.length === 0 && <p className="text-xs text-muted">Apps show up here while they play sound. You can also type a name.</p>}
    </>
  );
}

/** OBS audio source name, with suggestions while OBS is connected. */
export function ObsInputField({ value, onChange }: { value: string; onChange: (input: string) => void }) {
  const inventory = useResource(() => api.obsInventory());
  return (
    <>
      <ComboInput
        ariaLabel="OBS audio source"
        value={value}
        onChange={onChange}
        options={(inventory.data?.inputs ?? []).map((i) => ({ value: i, label: i }))}
        placeholder="Like Mic/Aux"
      />
      {inventory.error && <p className="text-xs text-muted">OBS isn't connected. Type the source name.</p>}
    </>
  );
}
