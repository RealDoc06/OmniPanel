// Saving to the host, kept free of the backend so it can be tested on its own.
// `createSaver` debounces saves per key and runs them one at a time, in order.
// Unload sends the latest snapshots immediately, with revisions to reject late older writes.
// `SettingsPatch` keeps unconfirmed settings edits field by field, so a host change
// to another field (a deck toggling the voice changer) survives a pending edit.
import type { Settings, SettingsPatch as HostSettingsPatch } from "@omni/protocol";

const ignore = () => {};

export interface SaveOptions {
  revision: number;
  unload?: boolean;
}

/** Per-key debounced saves, executed in order. `pending` holds values the host hasn't confirmed yet.
 * `onError` reports a failed save. `settle(current, sent)` returns what is still pending after
 * `sent` was stored (or failed); by default the key is done unless it changed since. */
export function createSaver<T>(
  save: (value: T, options: SaveOptions) => Promise<void>,
  onError: (error: unknown) => void,
  settle: (current: T, sent: T) => T | undefined = (current, sent) => (current === sent ? undefined : current),
) {
  const pending = new Map<string, T>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  /** Bumped by `cancel`, so a save queued before it never runs. */
  const generations = new Map<string, number>();
  /** Latest queued job per key, with the pending snapshot it was requested for. */
  const queued = new Map<string, { value: T; done: Promise<void> }>();
  let revision = 0;
  const revisions = new Map<string, number>();
  let chain: Promise<unknown> | undefined;

  /** Runs `job` after every save queued so far. */
  function enqueue<R>(job: () => Promise<R>): Promise<R> {
    // Start immediately when idle, so an explicit flush sends without an extra microtask.
    let result: Promise<R>;
    try {
      result = chain ? chain.then(job) : job();
    } catch (error) {
      result = Promise.reject(error);
    }
    const tail = result.catch(ignore);
    chain = tail;
    void tail.then(() => {
      if (chain === tail) chain = undefined;
    });
    return result;
  }

  /** Sends the pending value for `key` now. Resolves once the host has stored it, rejects if it failed. */
  function flush(key: string): Promise<void> {
    clearTimeout(timers.get(key));
    timers.delete(key);
    const value = pending.get(key);
    const last = queued.get(key);
    if (value === undefined || last?.value === value) return last?.done ?? Promise.resolve();
    const generation = generations.get(key) ?? 0;
    const done: Promise<void> = enqueue(async () => {
      try {
        if ((generations.get(key) ?? 0) !== generation) return;
        // Earlier jobs may have confirmed part of a settings patch while this one waited.
        // Send only what is still pending, rather than replaying the queued snapshot.
        const sent = pending.get(key);
        if (sent === undefined) return;
        try {
          await save(sent, { revision: revisions.get(key)! });
        } catch (error) {
          onError(error);
          throw error;
        } finally {
          if ((generations.get(key) ?? 0) === generation) {
            const current = pending.get(key);
            const rest = current === undefined ? undefined : settle(current, sent);
            if (rest === undefined) pending.delete(key);
            else pending.set(key, rest);
          }
        }
      } finally {
        if (queued.get(key)?.done === done) queued.delete(key);
      }
    });
    queued.set(key, { value, done });
    return done;
  }

  return {
    pending,
    isIdle: () => pending.size === 0 && queued.size === 0 && chain === undefined,
    flush,
    enqueue,
    schedule(key: string, value: T, delay: number) {
      pending.set(key, value);
      revisions.set(key, ++revision);
      clearTimeout(timers.get(key));
      if (delay > 0) timers.set(key, setTimeout(() => flush(key).catch(ignore), delay));
      else flush(key).catch(ignore);
    },
    /** Drops the pending value for `key`, including a save that is queued but not started. */
    cancel(key: string) {
      clearTimeout(timers.get(key));
      timers.delete(key);
      pending.delete(key);
      queued.delete(key);
      generations.set(key, (generations.get(key) ?? 0) + 1);
    },
    /** Waits for every pending save, including edits made during an in-flight save. */
    async flushAll() {
      while (pending.size > 0) await Promise.all([...pending.keys()].map(flush));
      await chain;
    },
    /** Unload cannot wait for responses. Send every latest snapshot immediately. */
    flushOnUnload() {
      for (const [key, value] of pending) {
        clearTimeout(timers.get(key));
        timers.delete(key);
        void save(value, { revision: revisions.get(key)!, unload: true }).catch(onError);
      }
    },
  };
}

/** Drains again when another saver gained edits or commands during the previous pass. */
export async function drainSavers(savers: readonly { flushAll(): Promise<void>; isIdle(): boolean }[]) {
  do {
    await Promise.all(savers.map((saver) => saver.flushAll()));
  } while (savers.some((saver) => !saver.isIdle()));
}

/** Quit always proceeds, even when saving fails or the host stops responding. */
export async function quitAfterFlush(flush: () => Promise<void>, quit: () => Promise<void>, timeoutMs = 3000) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      Promise.resolve().then(flush),
      new Promise<void>((resolve) => { timer = setTimeout(resolve, timeoutMs); }),
    ]);
  } catch {
    // Savers already report save errors; they must not prevent quitting.
  } finally {
    clearTimeout(timer);
    await quit();
  }
}

// ---------------------------------------------------------------------------
// Settings patches

/** Settings fields that are nested objects (usb, ai, obs, homeAssistant, voice). */
export type SettingsSection = { [K in keyof Settings]: Settings[K] extends object ? K : never }[keyof Settings];

/** Unconfirmed settings edits: top-level fields, and the edited fields of each nested section. */
export interface SettingsPatch {
  fields: Omit<HostSettingsPatch, SettingsSection>;
  sections: { [K in SettingsSection]: NonNullable<HostSettingsPatch[K]> };
}

export const emptyPatch = (): SettingsPatch => ({
  fields: {},
  sections: { usb: {}, ai: {}, obs: {}, homeAssistant: {}, voice: {} },
});

/** `settings` with the edits in `patch` on top. */
export function rebase(settings: Settings, { fields, sections }: SettingsPatch): Settings {
  // Typed as every section, so a new section in Settings fails to compile here until it is merged.
  const merged: Pick<Settings, SettingsSection> = {
    usb: { ...settings.usb, ...sections.usb },
    ai: { ...settings.ai, ...sections.ai },
    obs: { ...settings.obs, ...sections.obs },
    homeAssistant: { ...settings.homeAssistant, ...sections.homeAssistant },
    voice: { ...settings.voice, ...sections.voice },
  };
  return { ...settings, ...fields, ...merged };
}

/** Edits of `current` that `sent` didn't store: never sent, or changed since. */
function unsent<T extends object>(current: Partial<T>, sent: Partial<T>): Partial<T> {
  const rest: Partial<T> = {};
  for (const key in current) if (!Object.is(current[key], sent[key])) rest[key] = current[key];
  return rest;
}

/** `settle` for settings: drops only the fields whose sent values are still current. */
export function settlePatch(current: SettingsPatch, sent: SettingsPatch): SettingsPatch | undefined {
  const { sections: a } = current;
  const { sections: b } = sent;
  const rest: SettingsPatch = {
    fields: unsent(current.fields, sent.fields),
    sections: {
      usb: unsent(a.usb, b.usb),
      ai: unsent(a.ai, b.ai),
      obs: unsent(a.obs, b.obs),
      homeAssistant: unsent(a.homeAssistant, b.homeAssistant),
      voice: unsent(a.voice, b.voice),
    },
  };
  const empty = [rest.fields, ...Object.values(rest.sections)].every((edits) => Object.keys(edits).length === 0);
  return empty ? undefined : rest;
}
