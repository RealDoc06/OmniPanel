// Small React hooks for polling, async loading and throttling.
import { useCallback, useEffect, useEffectEvent, useRef, useState } from "react";
import { errorMessage } from "./util";

/**
 * Calls `tick` now and then every `ms` while `enabled` and the window is visible.
 * The next tick waits for the previous one, so slow IPC never piles up.
 */
export function usePolling(tick: () => Promise<unknown> | void, ms: number, enabled = true) {
  const onTick = useEffectEvent(tick);
  useEffect(() => {
    if (!enabled) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let stopped = false;
    const loop = async () => {
      if (document.visibilityState === "visible") {
        try {
          await onTick();
        } catch {
          // The caller shows errors where it makes sense; polling just keeps going.
        }
      }
      if (!stopped) timer = setTimeout(loop, ms);
    };
    void loop();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [ms, enabled]);
}

export interface Resource<T> {
  data: T | undefined;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

/** Loads data once (and again when `deps` change or `reload()` is called). */
export function useResource<T>(load: () => Promise<T>, deps: readonly unknown[] = []): Resource<T> {
  const [state, setState] = useState<{ data: T | undefined; error: string | null; loading: boolean }>({
    data: undefined,
    error: null,
    loading: true,
  });
  const [version, setVersion] = useState(0);
  const onLoad = useEffectEvent(load);
  useEffect(() => {
    let cancelled = false;
    setState((s) => ({ ...s, loading: true }));
    onLoad().then(
      (data) => !cancelled && setState({ data, error: null, loading: false }),
      (error: unknown) => !cancelled && setState({ data: undefined, error: errorMessage(error), loading: false }),
    );
    return () => {
      cancelled = true;
    };
  }, [version, ...deps]);
  const reload = useCallback(() => setVersion((v) => v + 1), []);
  return { ...state, reload };
}

/** Current time, refreshed every `ms`. */
export function useNow(ms = 1000): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(timer);
  }, [ms]);
  return now;
}

/** Throttles calls to at most one per `ms`; the last call always runs. */
export function useThrottled<A extends unknown[]>(fn: (...args: A) => void, ms: number): (...args: A) => void {
  const fnRef = useRef(fn);
  useEffect(() => {
    fnRef.current = fn;
  });
  const state = useRef<{ last: number; timer?: ReturnType<typeof setTimeout>; args?: A }>({ last: 0 });
  useEffect(() => () => clearTimeout(state.current.timer), []);
  return useCallback(
    (...args: A) => {
      const s = state.current;
      s.args = args;
      const wait = s.last + ms - Date.now();
      if (wait <= 0) {
        s.last = Date.now();
        fnRef.current(...args);
      } else if (!s.timer) {
        s.timer = setTimeout(() => {
          s.timer = undefined;
          s.last = Date.now();
          if (s.args) fnRef.current(...s.args);
        }, wait);
      }
    },
    [ms],
  );
}
