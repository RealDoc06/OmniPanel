// "Connect a device": the panel at the top of Devices (or, as `hero`, the whole first-run
// screen). It shows one QR code with its address, the 6-digit code (renewed by itself when it
// runs out) and a USB view for Android. Pairing is on while the panel is mounted; it closes
// itself when a device connects.
import type { PairingInfo } from "@omni/protocol";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Button, Disclosure, Notice, Section } from "../../components/ui";
import { api } from "../../lib/backend";
import { useNow, useResource } from "../../lib/hooks";
import { navigate, patchSection, run, useApp, useRuntime } from "../../lib/store";
import { toast, toastError } from "../../lib/toast";
import { errorMessage } from "../../lib/util";

const PLATFORM_TOOLS_URL = "https://developer.android.com/tools/releases/platform-tools";

// Start and stop reach the host one at a time, in call order, so StrictMode's mount,
// unmount and mount arrive as start, stop, start (T1-1). Only the latest start may set
// the code: `generation` goes up on every start and stop, and older results are dropped.
let queue: Promise<unknown> = Promise.resolve();
let generation = 0;

function enqueue<T>(command: () => Promise<T>): Promise<T> {
  const result = queue.then(command);
  queue = result.catch(() => {});
  return result;
}

export function ConnectPanel({ hero = false, onClose }: { hero?: boolean; onClose?: () => void }) {
  const [started, setStarted] = useState<PairingInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [usb, setUsb] = useState(false);
  const runtime = useRuntime();
  // The host pushes `runtime` after rotating the code on too many wrong guesses (T1-5).
  const info = runtime.pairing ?? started;

  const start = useCallback(() => {
    const id = ++generation;
    setError(null);
    enqueue(() => api.startPairing()).then(
      (next) => id === generation && setStarted(next),
      (e: unknown) => id === generation && setError(errorMessage(e)),
    );
  }, []);

  useEffect(() => {
    start();
    return () => {
      generation++;
      void enqueue(() => api.stopPairing()).catch(() => {});
    };
  }, [start]);

  // A code that runs out while the panel is open is replaced right away.
  const expiresAt = info?.expiresAt;
  useEffect(() => {
    if (expiresAt === undefined) return;
    const timer = setTimeout(start, Math.max(0, expiresAt - Date.now()));
    return () => clearTimeout(timer);
  }, [expiresAt, start]);

  // Close as soon as a device connects.
  const lastPaired = useApp((s) => s.lastPaired);
  const pairedAtOpen = useRef(lastPaired);
  useEffect(() => {
    if (lastPaired && lastPaired !== pairedAtOpen.current) onClose?.();
  }, [lastPaired, onClose]);

  const body = (
    <div className="flex flex-col gap-4">
      {error && <Notice tone="error">{error}</Notice>}
      {usb ? (
        <>
          <Code info={info} />
          <UsbSteps port={runtime.server.port} />
          <TextLink onClick={() => setUsb(false)}>Use Wi-Fi instead</TextLink>
        </>
      ) : (
        <>
          <WifiSteps urls={runtime.server.urls} info={info} />
          <p className="text-muted">
            The phone or tablet must be on the same Wi-Fi as this computer.{" "}
            <TextLink onClick={() => setUsb(true)}>Use a USB cable instead (Android)</TextLink>
          </p>
        </>
      )}
    </div>
  );

  if (hero) {
    return (
      <div className="flex flex-1 items-center justify-center px-7 py-10">
        <div className="w-full max-w-[640px]">
          <h2 className="mb-1 text-[20px] font-semibold tracking-[-0.01em]">Connect a phone or tablet</h2>
          <p className="mb-6 text-muted">It becomes a deck of buttons for this computer. Any browser works, even on old devices.</p>
          {body}
        </div>
      </div>
    );
  }
  return (
    <Section
      title="Connect a phone or tablet"
      actions={
        <Button size="sm" onClick={onClose}>
          Done
        </Button>
      }
    >
      {body}
    </Section>
  );
}

/** Step 1 (address and QR code) and step 2 (the code). */
function WifiSteps({ urls, info }: { urls: string[]; info: PairingInfo | null }) {
  const [first, ...others] = urls;
  return (
    <div className="flex items-start gap-8">
      <ol className="flex min-w-0 flex-1 flex-col gap-5">
        <Step n={1}>
          {first ? (
            <>
              <p className="flex min-h-7 flex-wrap items-center gap-x-2 gap-y-1">
                <span>
                  On the phone or tablet, open{" "}
                  <span data-selectable className="font-mono font-medium">
                    {hostOf(first)}
                  </span>{" "}
                  in its browser, or scan the code.
                </span>
                <Button size="sm" onClick={() => copyAddress(first)}>
                  Copy address
                </Button>
              </p>
              {others.length > 0 && (
                <div className="mt-1 max-w-sm">
                  <Disclosure label={`Other addresses (${others.length})`}>
                    <ul className="flex flex-col gap-1">
                      {others.map((url) => (
                        <li key={url} className="flex items-center gap-2">
                          <span data-selectable className="font-mono">
                            {hostOf(url)}
                          </span>
                          <Button size="sm" onClick={() => copyAddress(url)}>
                            Copy
                          </Button>
                        </li>
                      ))}
                    </ul>
                  </Disclosure>
                </div>
              )}
            </>
          ) : (
            <Notice tone="warn">No network address found. Connect this computer to Wi-Fi or Ethernet.</Notice>
          )}
        </Step>
        <Step n={2}>
          <p className="min-h-7 leading-7">Type this code on it.</p>
          <Code info={info} />
        </Step>
      </ol>
      {first && <QrCode text={first} />}
    </div>
  );
}

/** USB steps for Android (adb reverse), or what's missing for them. */
function UsbSteps({ port }: { port: number }) {
  const usb = useRuntime().usb;
  const failure = usb.error ? <Notice tone="error">{usb.error}</Notice> : null;
  if (!usb.enabled) {
    return (
      <div className="flex items-center gap-3">
        <Notice>USB connection is off.</Notice>
        <Button size="sm" onClick={() => patchSection("usb", { enabled: true })}>
          Turn on
        </Button>
      </div>
    );
  }
  if (!usb.adbPath) {
    return (
      <>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <Notice tone="warn">USB needs Android platform-tools.</Notice>
          <Button size="sm" onClick={() => void run(api.runAction({ type: "open", target: PLATFORM_TOOLS_URL }))}>
            Download
          </Button>
          or set its location in
          <TextLink onClick={() => navigate("settings", "usb")}>Settings &gt; Advanced</TextLink>
        </div>
        {failure}
      </>
    );
  }
  return (
    <>
      {failure}
      <ol className="flex flex-col gap-2">
        <Step n={1}>
          On the Android device, turn on USB debugging: Settings &gt; About phone &gt; tap Build number 7 times, then Settings
          &gt; Developer options &gt; USB debugging.
        </Step>
        <Step n={2}>Plug it into this computer and tap Allow on the device.</Step>
        <Step n={3}>
          On the device, open{" "}
          <span data-selectable className="font-mono">
            localhost:{port}
          </span>{" "}
          in the browser and type the code above.
        </Step>
      </ol>
      {usb.devices.length > 0 && (
        <ul className="ml-6 divide-y divide-line border-y border-line">
          {usb.devices.map((d) => (
            <li key={d.serial} className="flex items-center gap-3 py-2">
              <span title={d.serial} className="font-medium">
                {d.model || d.serial}
              </span>
              <span className="ml-auto">
                {d.state === "unauthorized" ? (
                  <span className="text-warning">Tap Allow on the device</span>
                ) : d.state !== "device" ? (
                  <span className="text-warning">Not ready ({d.state})</span>
                ) : d.forwarded ? (
                  <span className="text-success">Ready</span>
                ) : (
                  <span className="text-muted">Connecting…</span>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/** The 6-digit code, big and selectable, with the time until it renews. */
function Code({ info }: { info: PairingInfo | null }) {
  // Tick each second, but also use fresh time when a replacement code arrives between ticks.
  useNow(1000);
  const left = info ? Math.max(0, Math.ceil((info.expiresAt - Date.now()) / 1000)) : 0;
  return (
    <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 pt-1">
      <span data-selectable aria-live="polite" className="font-mono text-[36px] leading-none font-semibold tracking-[0.16em] tabular-nums">
        {info ? `${info.code.slice(0, 3)} ${info.code.slice(3)}` : "··· ···"}
      </span>
      {info && (
        <span className="text-xs text-muted tabular-nums">
          New code in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}
        </span>
      )}
    </div>
  );
}

function Step({ n, children }: { n: number; children: ReactNode }) {
  return (
    <li className="flex gap-3">
      <span className="w-3 shrink-0 leading-7 font-semibold tabular-nums">{n}</span>
      <div className="min-w-0 flex-1">{children}</div>
    </li>
  );
}

function TextLink({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button type="button" className="w-fit text-fg hover:underline" onClick={onClick}>
      {children}
    </button>
  );
}

function QrCode({ text }: { text: string }) {
  const qr = useResource(() => api.qrSvg(text), [text]);
  return (
    <div
      role="img"
      aria-label={`QR code for ${hostOf(text)}`}
      className="size-[176px] shrink-0 rounded-ui bg-white p-2.5 [&>svg]:size-full"
      dangerouslySetInnerHTML={qr.data ? { __html: qr.data } : undefined}
    />
  );
}

/** "192.168.1.24:7710" from a deck URL. */
const hostOf = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/$/, "");

function copyAddress(url: string) {
  navigator.clipboard.writeText(url).then(() => toast("Address copied", "success"), toastError);
}
