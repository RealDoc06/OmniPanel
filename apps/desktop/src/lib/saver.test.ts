import assert from "node:assert/strict";
import { test } from "node:test";
import type { Settings } from "@omni/protocol";
import { createSaver, drainSavers, emptyPatch, quitAfterFlush, rebase, settlePatch, type SettingsPatch } from "./saver.ts";

const fail = (error: unknown) => assert.fail(String(error));
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A promise the test resolves by hand. */
function gate() {
  let open = () => {};
  const promise = new Promise<void>((resolve) => (open = resolve));
  return { promise, open };
}

const SETTINGS: Settings = {
  hostName: "Desk",
  port: 7710,
  launchAtLogin: false,
  startHidden: false,
  themeId: "midnight",
  defaultProfileId: null,
  usb: { enabled: true, adbPath: null },
  ai: { provider: "openai", model: "", baseUrl: null, apiKey: "" },
  obs: { enabled: false, host: "127.0.0.1", port: 4455, password: "" },
  homeAssistant: { enabled: false, url: "", token: "" },
  voice: {
    enabled: false,
    presetId: "clean",
    inputDevice: null,
    outputDevice: null,
    monitor: false,
    monitorDevice: null,
    inputGainDb: 0,
    outputGainDb: 0,
    noiseGate: false,
    gateThresholdDb: -50,
  },
};

test("a host change to voice.enabled survives while hostName is pending", async () => {
  let host = SETTINGS;
  const sent: Settings[] = [];
  const firstSave = gate();
  // Like the store: a save sends the patch rebased onto the latest settings.
  const saver = createSaver<SettingsPatch>(async (patch) => {
    host = rebase(host, patch);
    sent.push(host);
    if (sent.length === 1) await firstSave.promise;
  }, fail, settlePatch);
  saver.schedule("settings", { ...emptyPatch(), fields: { hostName: "Studio" } }, 500);

  host = { ...host, voice: { ...host.voice, enabled: true } }; // a deck turns the voice changer on
  const patch = saver.pending.get("settings");
  assert.ok(patch);
  assert.equal(rebase(host, patch).voice.enabled, true);

  const stored = saver.flush("settings");
  await tick();
  assert.equal(sent.length, 1);
  assert.equal(sent[0]?.hostName, "Studio");
  assert.equal(sent[0]?.voice.enabled, true);

  // A second edit waits behind the first save. Once hostName is confirmed, it must
  // no longer be part of that queued patch or overwrite a subsequent host change.
  saver.schedule("settings", { ...patch, sections: { ...patch.sections, usb: { adbPath: "/test/adb" } } }, 0);
  host = { ...host, hostName: "Changed elsewhere" };
  firstSave.open();
  await stored;
  await saver.flush("settings");
  assert.equal(sent[1]?.hostName, "Changed elsewhere");
  assert.equal(sent[1]?.voice.enabled, true);
  assert.equal(sent[1]?.usb.adbPath, "/test/adb");
  assert.equal(saver.pending.size, 0);
});

test("a save that was queued and then cancelled never runs", async () => {
  const slow = gate();
  const saved: string[] = [];
  const saver = createSaver<string>(async (value) => {
    saved.push(value);
    if (value === "slow") await slow.promise;
  }, fail);
  saver.schedule("other", "slow", 0);
  await tick();
  saver.schedule("profile", "edited", 0); // queued behind the slow save
  saver.cancel("profile"); // the profile was deleted
  slow.open();
  await saver.enqueue(async () => {});
  assert.deepEqual(saved, ["slow"]);
});

test("flush(key) resolves after save resolves", async () => {
  const host = gate();
  let stored = false;
  const saver = createSaver<string>(async () => {
    await host.promise;
    stored = true;
  }, fail);
  saver.schedule("profile", "edited", 400);
  let flushed = false;
  const done = saver.flush("profile").then(() => (flushed = true));
  await tick();
  assert.equal(flushed, false);
  host.open();
  await done;
  assert.equal(stored, true);
});

test("unload sends every latest snapshot immediately while an older save is in flight", async () => {
  const first = gate();
  const sent: { value: string; revision: number; unload: boolean }[] = [];
  const saver = createSaver<string>(async (value, options) => {
    sent.push({ value, revision: options.revision, unload: options.unload ?? false });
    if (sent.length === 1) await first.promise;
  }, fail);
  saver.schedule("profile", "FIRST", 0);
  saver.schedule("profile", "LATEST", 400);
  saver.schedule("other", "QUEUED", 0);
  saver.flushOnUnload();
  assert.deepEqual(sent.map(({ value, unload }) => ({ value, unload })), [
    { value: "FIRST", unload: false },
    { value: "LATEST", unload: true },
    { value: "QUEUED", unload: true },
  ]);
  assert.ok(sent[1]!.revision > sent[0]!.revision);
  first.open();
  await saver.flushAll();
  assert.equal(saver.pending.size, 0);
});

test("complete flush waits for in-flight work and edits added while it waits", async () => {
  const first = gate();
  const latest = gate();
  const sent: string[] = [];
  const saver = createSaver<string>(async (value) => {
    sent.push(value);
    await (value === "FIRST" ? first.promise : latest.promise);
  }, fail);
  saver.schedule("profile", "FIRST", 0);
  let done = false;
  const flushed = saver.flushAll().then(() => { done = true; });
  saver.schedule("profile", "LATEST", 500);
  first.open();
  await tick();
  assert.deepEqual(sent, ["FIRST", "LATEST"]);
  assert.equal(done, false);
  latest.open();
  await flushed;
  assert.equal(done, true);
  assert.equal(saver.pending.size, 0);
});


test("quit proceeds after a rejected save", async () => {
  let quit = false;
  const errors: unknown[] = [];
  const saver = createSaver<string>(async () => { throw new Error("save failed"); }, (error) => errors.push(error));
  saver.schedule("profile", "edited", 500);
  await quitAfterFlush(() => saver.flushAll(), async () => { quit = true; });
  assert.equal(errors.length, 1);
  assert.equal(quit, true);
});

test("quit proceeds when saving never settles", async () => {
  let quit = false;
  await quitAfterFlush(() => new Promise<void>(() => {}), async () => { quit = true; }, 10);
  assert.equal(quit, true);
});

test("overall drain revisits idle savers for new edits and queued commands", async () => {
  const profile = gate();
  const command = gate();
  const settings = createSaver<string>(async (value) => { saved.push(value); }, fail);
  const profiles = createSaver<string>(() => profile.promise, fail);
  const saved: string[] = [];
  profiles.schedule("profile", "FIRST", 0);
  let done = false;
  const flushed = drainSavers([settings, profiles]).then(() => { done = true; });
  await tick(); // The settings saver has already finished its first pass.
  settings.schedule("settings", "LATEST", 500);
  void settings.enqueue(() => command.promise);
  profile.open();
  await tick();
  assert.equal(done, false);
  command.open();
  await flushed;
  assert.deepEqual(saved, ["LATEST"]);
  assert.ok(settings.isIdle());
  assert.ok(profiles.isIdle());
});
