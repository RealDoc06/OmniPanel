// "Share app sound" section of the Sound screen: mixes the chosen apps into the voice
// output (the virtual microphone) next to the user's voice, so a call hears them too.
// Starting and stopping run the same voice action as deck buttons, one app at a time,
// so a click here and a press on a deck cannot undo each other.
import type { VoiceOp } from "@omni/protocol";
import { useState } from "react";
import { I } from "../../components/Icon";
import { Button, ComboInput, Field, Notice, Range, Section } from "../../components/ui";
import { recordsOnly } from "../../lib/actions";
import { api } from "../../lib/backend";
import { useResource } from "../../lib/hooks";
import { navigate, patchSection, run, useIsMac, useRuntime, useSettings } from "../../lib/store";
import { cx } from "../../lib/util";

const db = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(0)} dB`;

/** macOS privacy pane for "Screen & System Audio Recording", which capturing an app needs. */
const AUDIO_PERMISSION_PANE = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

const STATE_TONES = { Off: "text-muted", Sharing: "text-success", "Needs setup": "text-warning", Stopped: "text-danger", "Not supported": "text-muted" };

export function ShareAppsSection() {
  const { voice } = useSettings();
  const status = useRuntime().voice;
  const mac = useIsMac();
  // Suggestions only: the host matches typed names too, and an app can be shared before it plays.
  const mixer = useResource(() => api.getMixer());
  const [app, setApp] = useState("");
  const shared = voice.sharedApps;
  const isShared = (name: string) => shared.some((a) => a.toLowerCase() === name.toLowerCase());
  const share = (type: Extract<VoiceOp, { app: string }>["type"], name: string) => void run(api.runAction({ type: "voice", op: { type, app: name } }));
  const name = app.trim();

  const state: keyof typeof STATE_TONES = status.shareUnsupported
    ? "Not supported"
    : shared.length === 0 ? "Off" : voice.outputDevice === null ? "Needs setup" : status.running && !status.shareError ? "Sharing" : "Stopped";

  return (
    <Section title="Share app sound" id="sound-share" actions={<span className={cx("text-xs", STATE_TONES[state])}>{state}</span>}>
      <p className="mb-2 text-muted">
        Lets people in your call hear an app, like music, a video or a game, along with your voice. You keep hearing it as usual.
      </p>
      {status.shareUnsupported ? (
        <Notice tone="warn">{status.shareUnsupported}</Notice>
      ) : (
        <>
          {voice.outputDevice === null && (
            <div className="mb-1 flex flex-wrap items-center gap-x-3 gap-y-1">
              <Notice tone="warn">Shared sound goes to the voice changer's virtual microphone, which needs a one-time setup.</Notice>
              <Button size="sm" onClick={() => navigate("sound", "voice")}>
                Set up
              </Button>
            </div>
          )}
          {(state === "Stopped" || status.shareError) && (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 py-1">
              <Notice tone="error">Sharing stopped{status.shareError || status.error ? `: ${status.shareError ?? status.error}` : "."}</Notice>
              {mac && status.shareError?.includes("System Audio Recording") && (
                <Button size="sm" onClick={() => void run(api.runAction({ type: "open", target: AUDIO_PERMISSION_PANE }))}>
                  Open System Settings
                </Button>
              )}
            </div>
          )}
          {shared.length > 0 && (
            <Field label="Sharing">
              <ul className="flex flex-col">
                {shared.map((a) => (
                  <li key={a} className="flex h-8 items-center gap-2">
                    <I name="cast" className="size-4 shrink-0 text-muted" />
                    <span className="w-44 min-w-0 truncate">{a}</span>
                    <Button size="sm" variant="ghost" className="text-muted hover:text-fg" onClick={() => share("unshareApp", a)}>
                      Stop sharing
                    </Button>
                    {state === "Sharing" && status.shareWaiting.includes(a) && <span className="text-xs text-muted">not playing anything yet</span>}
                  </li>
                ))}
              </ul>
            </Field>
          )}
          <Field
            label="Share an app"
            hint={
              <>
                {mixer.data?.apps.length === 0 && "Apps show up here while they play sound. You can also type the app's name. "}
                Don't share the app your call is in: the others would hear themselves.
              </>
            }
          >
            <form
              className="flex items-center gap-2"
              // Apps come and go: look again whenever the field is used.
              onFocus={mixer.reload}
              onSubmit={(e) => {
                e.preventDefault();
                if (name && !isShared(name)) share("shareApp", name);
                setApp("");
              }}
            >
              <ComboInput
                ariaLabel="App to share"
                className="max-w-xs"
                value={app}
                onChange={setApp}
                options={(mixer.data?.apps ?? []).filter((a) => !recordsOnly(a) && !isShared(a.name)).map((a) => ({ value: a.name, label: a.name }))}
                placeholder="Pick or type an app"
              />
              <Button size="sm" type="submit" icon="cast" disabled={!name || isShared(name)}>
                Share
              </Button>
            </form>
          </Field>
          <Field label="Volume in the call">
            <Range
              label="Volume in the call"
              className="max-w-xs"
              min={-30}
              max={12}
              step={1}
              value={voice.shareGainDb}
              format={db}
              onChange={(shareGainDb) => patchSection("voice", { shareGainDb }, 150)}
            />
          </Field>
        </>
      )}
    </Section>
  );
}
