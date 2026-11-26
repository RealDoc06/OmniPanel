import assert from "node:assert/strict";
import { test } from "node:test";
import type { Action, AppAudio, Control } from "@omni/protocol";
import { ACTION_TYPES, describeAction, findDevice, liveStateTitle } from "./actions.ts";
import { emptyStyle } from "./controls.ts";
import { RECIPES, applyAutoLabel, applyPick, dynamicRecipes, isAutoLabel, searchRecipes } from "./recipes.ts";

const spotify: AppAudio = { id: "com.spotify.client", name: "Spotify", pid: 1, volume: 1, muted: false, playing: true, output: { type: "default" }, input: null };
const dynamic = dynamicRecipes([{ name: "Spotify", launch: "Spotify" }], [spotify]);
const first = (query: string) => searchRecipes(query, dynamic)[0]?.id;

test("every action type except delay has a static recipe", () => {
  const made = new Set(RECIPES.map((r) => r.make()).flatMap((kind) => (kind.type === "button" && kind.action ? [kind.action.type] : [])));
  assert.deepEqual(
    ACTION_TYPES.filter((t) => t !== "delay" && !made.has(t)),
    [],
  );
});

test("ranks the obvious entry first", () => {
  assert.equal(first("mute my mic"), "mic-mute");
  assert.equal(first("shortcut"), "press-keys");
  assert.equal(first("open spotify"), "open:Spotify");
  assert.equal(first("share spotify"), "share:Spotify");
  assert.equal(first("hear my music on a call"), "share-app");
});

test("tells recipe labels from typed ones", () => {
  assert.equal(isAutoLabel("Mute mic"), true);
  assert.equal(isAutoLabel("Spotify"), true);
  assert.equal(isAutoLabel("Mute Spotify"), true);
  assert.equal(isAutoLabel("Hello"), false);
});

test("search requires every word and only includes apps while searching", () => {
  assert.equal(searchRecipes("", dynamic).some((r) => r.name), false);
  assert.deepEqual(searchRecipes("spotify", dynamic).slice(0, 3).map((r) => r.label), [
    "Open Spotify", "Mute / unmute Spotify", "Spotify volume slider",
  ]);
  assert.deepEqual(searchRecipes("spotify unknown", dynamic), []);
  assert.equal(searchRecipes("please", dynamic).some((r) => r.name), true);
});

const button = (action: Action): Control["kind"] => ({ type: "button", action });

test("share buttons are labelled after their app and what they do", () => {
  const share = (type: "toggleShareApp" | "unshareApp", app: string) => button({ type: "voice", op: { type, app } });
  const style = applyAutoLabel({ ...emptyStyle(), label: "Share app" }, share("toggleShareApp", ""), share("toggleShareApp", "Spotify"));
  assert.equal(style.label, "Share Spotify");
  assert.equal(applyAutoLabel(style, share("toggleShareApp", "Spotify"), share("unshareApp", "Spotify")).label, "Stop sharing Spotify");
});

test("automatic labels follow app, website and navigation targets across repeated edits", () => {
  const emptyApp = button({ type: "openApp", app: "", args: [] });
  const safari = button({ type: "openApp", app: "/Applications/Safari.app", args: [] });
  const terminal = button({ type: "openApp", app: "/Applications/Utilities/Terminal.app", args: [] });
  const style = applyAutoLabel({ ...emptyStyle(), label: "Open app" }, emptyApp, safari);
  assert.equal(style.label, "Safari");
  assert.equal(applyAutoLabel(style, safari, terminal).label, "Terminal");
  const custom = { ...style, label: "Start my day" };
  assert.equal(applyAutoLabel(custom, safari, terminal), custom);

  const site = button({ type: "open", target: "https://example.com/path" });
  const otherSite = button({ type: "open", target: "https://example.org/other" });
  const website = applyAutoLabel({ ...emptyStyle(), label: "Website" }, button({ type: "open", target: "https://" }), site);
  assert.equal(website.label, "example.com");
  assert.equal(applyAutoLabel(website, site, otherSite).label, "example.org");
  assert.equal(applyPick(website, { kind: emptyApp, style: { label: "Open app" } }, site).label, "Open app");
  assert.equal(applyPick({ ...website, label: "My site" }, { kind: emptyApp, style: { label: "Open app" } }, site).label, "My site");
  const names = { page: (id: string) => id === "messages" ? "Messages" : undefined, profile: () => "Streaming" };
  const page = button({ type: "navigate", to: { type: "page", pageId: "messages" } });
  const layout = button({ type: "navigate", to: { type: "profile", profileId: "streaming" } });
  assert.equal(applyAutoLabel({ ...emptyStyle(), label: "Page" }, page, page, names).label, "Messages");
  assert.equal(applyAutoLabel({ ...emptyStyle(), label: "Layout" }, layout, layout, names).label, "Streaming");
});

test("automatic labels describe other recipe targets without replacing custom labels", () => {
  const cases: { label: string; kind: Control["kind"]; expected: string }[] = [
    { label: "Folder", kind: button({ type: "open", target: "/Users/doc/Documents" }), expected: "Documents" },
    { label: "Mute app", kind: button({ type: "volume", target: { type: "app", app: "Spotify" }, op: { type: "toggleMute" } }), expected: "Mute Spotify" },
    { label: "Volume 50%", kind: button({ type: "volume", target: { type: "master" }, op: { type: "set", value: 0.75 } }), expected: "Volume 75%" },
    { label: "App", kind: { type: "slider", vertical: true, target: { type: "app", app: "Discord" } }, expected: "Discord" },
    { label: "Scene", kind: button({ type: "obs", op: { type: "setScene", scene: "Camera" } }), expected: "Camera" },
    { label: "Robot", kind: button({ type: "voice", op: { type: "setPreset", presetId: "chipmunk" } }), expected: "Chipmunk" },
    { label: "Speaker", kind: button({ type: "appDevice", app: "Spotify", direction: "output", device: "bluez_output.1" }), expected: "Headphones" },
    { label: "Light", kind: button({ type: "homeAssistant", op: { type: "toggle", entityId: "light.desk" } }), expected: "Desk light" },
  ];
  const names = { preset: () => "Chipmunk", entity: () => "Desk light", device: () => "Headphones" };
  for (const { label, kind, expected } of cases) {
    assert.equal(applyAutoLabel({ ...emptyStyle(), label }, kind, kind, names).label, expected);
    assert.equal(applyAutoLabel({ ...emptyStyle(), label: "My own label" }, kind, kind, names).label, "My own label");
  }
});

test("app device actions read as plain sentences and light up only for a device", () => {
  const names = { device: (id: string) => (id === "bluez_output.1" ? "Headphones" : undefined) };
  const to = (direction: "output" | "input", device: string | null): Action => ({ type: "appDevice", app: "Spotify", direction, device });
  assert.equal(describeAction(to("output", "bluez_output.1"), names), "Play Spotify on Headphones");
  assert.equal(describeAction(to("output", null), names), "Play Spotify on the default speaker");
  assert.equal(describeAction(to("input", "Shure MV7"), names), "Use Shure MV7 for Spotify");
  assert.equal(liveStateTitle(button(to("output", "bluez_output.1"))), "While it plays there");
  assert.equal(liveStateTitle(button(to("output", null))), null);
});

test("an app that only records gets no mute, volume or share entries", () => {
  const obs: AppAudio = { ...spotify, id: "obs", name: "OBS", playing: false, output: null, input: { type: "default" } };
  assert.deepEqual(dynamicRecipes([], [spotify, obs]).map((r) => r.id), ["mute:Spotify", "slider:Spotify", "share:Spotify"]);
});

test("a device is found by id or by name, like the host finds it", () => {
  const devices = [
    { id: "alsa_output.usb-0", name: "Scarlett Solo", isDefault: true, isVirtual: false },
    { id: "bluez_output.1", name: "WH-1000XM4 Headphones", isDefault: false, isVirtual: false },
  ];
  const id = (query: string) => findDevice(devices, query)?.id;
  assert.equal(id("BLUEZ_OUTPUT.1"), "bluez_output.1");
  assert.equal(id("scarlett solo"), "alsa_output.usb-0");
  assert.equal(id(" Headphones "), "bluez_output.1");
  assert.equal(id("HDMI"), undefined);
  assert.equal(id(""), undefined);
});
