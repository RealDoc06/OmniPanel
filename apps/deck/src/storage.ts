// Pairing token persistence: localStorage, with a cookie fallback for browsers
// where it throws (iOS private mode, disabled storage).

const KEY = "omnipanel.token";

function readCookie(): string | null {
  const parts = document.cookie.split(";");
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i].replace(/^\s+/, "");
    if (part.indexOf(KEY + "=") === 0) return decodeURIComponent(part.slice(KEY.length + 1));
  }
  return null;
}

function writeCookie(value: string, maxAge: number): void {
  document.cookie = KEY + "=" + encodeURIComponent(value) + "; max-age=" + maxAge + "; path=/";
}

export function loadToken(): string | null {
  try {
    const value = window.localStorage.getItem(KEY);
    if (value) return value;
  } catch (_e) {
    // fall through to the cookie
  }
  return readCookie() || null;
}

export function saveToken(token: string): void {
  try {
    window.localStorage.setItem(KEY, token);
    return;
  } catch (_e) {
    // fall through to the cookie
  }
  writeCookie(token, 60 * 60 * 24 * 365 * 10);
}

export function clearToken(): void {
  try {
    window.localStorage.removeItem(KEY);
  } catch (_e) {
    // ignore
  }
  writeCookie("", 0);
}
