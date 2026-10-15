// Builds packages/icons/icons.json (name -> inner SVG markup, single-quoted
// attributes so the host can borrow it without unescaping) and tags.json
// (name -> search keywords) from an extracted lucide-static package.
// Usage: node scripts/gen-icons.mjs <path/to/lucide-static/package>
import { copyFileSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const src = process.argv[2];
if (!src) throw new Error("usage: gen-icons.mjs <lucide-static package dir>");
const out = new URL("../packages/icons/", import.meta.url).pathname;

const icons = {};
for (const file of readdirSync(join(src, "icons")).sort()) {
  if (!file.endsWith(".svg")) continue;
  const svg = readFileSync(join(src, "icons", file), "utf8");
  const inner = svg
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/^[\s\S]*?<svg[^>]*>/, "")
    .replace(/<\/svg>\s*$/, "")
    .replace(/\s*\n\s*/g, "")
    .replace(/\s*\/>/g, "/>")
    .replace(/"/g, "'");
  icons[file.slice(0, -4)] = inner;
}
const tags = JSON.parse(readFileSync(join(src, "tags.json"), "utf8"));
const keptTags = Object.fromEntries(Object.keys(icons).map((n) => [n, tags[n] ?? []]));

writeFileSync(join(out, "icons.json"), JSON.stringify(icons));
writeFileSync(join(out, "tags.json"), JSON.stringify(keptTags));
copyFileSync(join(src, "LICENSE"), join(out, "LICENSE-lucide"));
console.log(`icons: ${Object.keys(icons).length}`);
