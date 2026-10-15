// "Pair a device": shows the 6-digit code with its countdown, deck URLs with QR codes,
// and the USB (adb reverse) status. Closes by itself when a device pairs.
import type { PairingInfo } from "@omni/protocol";
import { useCallback, useEffect, useRef, useState } from "react";
import { I } from "../../components/Icon";
import { Button, Modal, Notice, Segmented } from "../../components/ui";
import { api } from "../../lib/backend";
import { useNow, useResource } from "../../lib/hooks";
import { navigate, patchSection, useApp, useRuntime } from "../../lib/store";
import { errorMessage } from "../../lib/util";

type Tab = "wifi" | "usb";

export function PairDialog({ onClose }: { onClose: () => void }) {
  const [info, setInfo] = useState<PairingInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("wifi");
  const server = useRuntime().server;

  const start = useCallback(() => {
    setError(null);
    api.startPairing().then(setInfo, (e: unknown) => setError(errorMessage(e)));
  }, []);

  useEffect(() => {
    start();
    return () => void api.stopPairing().catch(() => {});
  }, [start]);

  // Close as soon as a device pairs.
  const lastPaired = useApp((s) => s.lastPaired);
  const pairedAtOpen = useRef(lastPaired);
  useEffect(() => {
    if (lastPaired && lastPaired !== pairedAtOpen.current) onClose();
  }, [lastPaired, onClose]);

  return (
    <Modal title="Pair a device" width={640} onClose={onClose}>
      <div className="flex flex-col gap-4 p-5">
        {server.error && <Notice tone="error">Deck server: {server.error}</Notice>}
        {error && <Notice tone="error">{error}</Notice>}
        <Code info={info} onRenew={start} />
        <Segmented
          label="Connection"
          value={tab}
          onChange={setTab}
          options={[
            { value: "wifi", label: "Wi-Fi", icon: "wifi" },
            { value: "usb", label: "USB (Android)", icon: "usb" },
          ]}
        />
        <div className="min-h-[236px]">
          {tab === "wifi" ? <WifiPanel urls={server.urls} /> : <UsbPanel port={server.port} onClose={onClose} />}
        </div>
      </div>
    </Modal>
  );
}

function Code({ info, onRenew }: { info: PairingInfo | null; onRenew: () => void }) {
  const now = useNow(1000);
  const left = info ? Math.max(0, Math.round((info.expiresAt - now) / 1000)) : 0;
  const expired = info !== null && left === 0;
  return (
    <div className="flex items-end gap-6">
      <div>
        <div className="text-xs text-muted">Pairing code</div>
        <div
          data-selectable
          aria-live="polite"
          className={`font-mono text-[44px] leading-none font-semibold tracking-[0.18em] tabular-nums ${expired ? "text-muted line-through" : ""}`}
        >
          {info ? `${info.code.slice(0, 3)} ${info.code.slice(3)}` : "··· ···"}
        </div>
      </div>
      <div className="pb-1 text-muted">
        {info && !expired && (
          <span className="tabular-nums">
            Expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}
          </span>
        )}
        {expired && (
          <Button size="sm" icon="refresh-cw" onClick={onRenew}>
            New code
          </Button>
        )}
      </div>
    </div>
  );
}

function WifiPanel({ urls }: { urls: string[] }) {
  if (urls.length === 0) {
    return <Notice tone="warn">No network address found. Connect this computer to Wi-Fi or Ethernet.</Notice>;
  }
  return (
    <div className="flex flex-col gap-3">
      <ol className="list-inside list-decimal text-fg">
        <li>On the phone or tablet (same network), scan a code or open the address.</li>
        <li>Enter the pairing code.</li>
      </ol>
      <div className="flex flex-wrap gap-4">
        {urls.map((url) => (
          <figure key={url} className="flex w-[168px] flex-col items-center gap-2">
            <QrCode text={url} />
            <figcaption data-selectable className="w-full truncate text-center font-mono text-xs" title={url}>
              {url.replace(/^https?:\/\//, "")}
            </figcaption>
          </figure>
        ))}
      </div>
    </div>
  );
}

function QrCode({ text }: { text: string }) {
  const qr = useResource(() => api.qrSvg(text), [text]);
  return (
    <div
      role="img"
      aria-label={`QR code for ${text}`}
      className="size-[168px] rounded-ui-sm bg-white p-2 [&>svg]:size-full"
      dangerouslySetInnerHTML={qr.data ? { __html: qr.data } : undefined}
    />
  );
}

function UsbPanel({ port, onClose }: { port: number; onClose: () => void }) {
  const usb = useRuntime().usb;
  if (!usb.enabled) {
    return (
      <div className="flex items-center gap-3">
        <p className="flex-1">USB forwarding is off.</p>
        <Button onClick={() => patchSection("usb", { enabled: true })}>Turn on</Button>
      </div>
    );
  }
  if (!usb.adbPath) {
    return (
      <Notice tone="warn">
        adb was not found. Install Android platform-tools, or set the adb path in{" "}
        <button
          type="button"
          className="underline"
          onClick={() => {
            onClose();
            navigate("settings", "usb");
          }}
        >
          Settings
        </button>
        .{usb.error && <div className="mt-1">{usb.error}</div>}
      </Notice>
    );
  }
  const local = `http://localhost:${port}`;
  return (
    <div className="flex flex-col gap-3">
      {usb.error && <Notice tone="error">{usb.error}</Notice>}
      {usb.devices.length === 0 ? (
        <p className="text-fg">Connect an Android device with USB debugging enabled.</p>
      ) : (
        <ul className="divide-y divide-line border-y border-line">
          {usb.devices.map((d) => (
            <li key={d.serial} className="flex items-center gap-3 py-2">
              <I name="smartphone" className="size-4 text-muted" />
              <span className="font-medium">{d.model || d.serial}</span>
              <span className="text-xs text-muted">{d.serial}</span>
              <span className="ml-auto text-xs">
                {d.state === "unauthorized" ? (
                  <span className="text-warning">Allow USB debugging on the device</span>
                ) : d.state !== "device" ? (
                  <span className="text-warning">{d.state}</span>
                ) : d.forwarded ? (
                  <span className="text-success">Forwarded</span>
                ) : (
                  <span className="text-muted">Forwarding…</span>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
      <p>
        Then open <span data-selectable className="font-mono">{local}</span> in the device browser and enter the code.
      </p>
      <p className="text-xs text-muted">adb: {usb.adbPath}</p>
    </div>
  );
}
