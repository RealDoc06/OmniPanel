import type { ClientInfo } from "@omni/protocol";

interface IosNavigator extends Navigator {
  standalone?: boolean;
}

/** Protocol version this client speaks (see PROTOCOL_VERSION in omni-core). */
export const PROTOCOL_VERSION = 1;

/** Friendly default for the pairing "device name" field, derived from the user agent. */
export function defaultDeviceName(): string {
  const ua = navigator.userAgent;
  // iPadOS 13+ Safari pretends to be a Mac but has a touch screen.
  if (/iPad/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1)) return "iPad";
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPod/.test(ua)) return "iPod";
  if (/Kindle|Silk/.test(ua)) return "Kindle Fire";
  if (/Android/.test(ua)) return /Mobile/.test(ua) ? "Android phone" : "Android tablet";
  if (/CrOS/.test(ua)) return "Chromebook";
  if (/Windows Phone/.test(ua)) return "Windows phone";
  if (/Windows/.test(ua)) return "Windows PC";
  if (/Macintosh/.test(ua)) return "Mac";
  if (/Linux/.test(ua)) return "Linux PC";
  return "Browser";
}

export function viewportSize(): { width: number; height: number } {
  return {
    width: window.innerWidth || document.documentElement.clientWidth,
    height: window.innerHeight || document.documentElement.clientHeight,
  };
}

export function clientInfo(): ClientInfo {
  const size = viewportSize();
  return {
    protocol: PROTOCOL_VERSION,
    userAgent: navigator.userAgent,
    width: size.width,
    height: size.height,
    pixelRatio: window.devicePixelRatio || 1,
  };
}

/** iPhone, iPod or iPad (iPadOS 13+ Safari reports a Mac but has a touch screen). */
export function isIos(): boolean {
  const ua = navigator.userAgent;
  return /iPad|iPhone|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1);
}

/** Major iOS version from the user agent ("OS 9_3"); 0 when unknown (iPadOS 13+ hides it). */
export function iosVersion(): number {
  const match = /OS (\d+)_/.exec(navigator.userAgent);
  return match ? parseInt(match[1], 10) : 0;
}

export function isAndroid(): boolean {
  return /Android/.test(navigator.userAgent);
}

/** True when launched from the iOS home screen (no browser chrome). */
export function isIosStandalone(): boolean {
  return (navigator as IosNavigator).standalone === true;
}
