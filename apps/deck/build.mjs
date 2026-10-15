// Builds the deck client into dist/: esbuild bundles to one IIFE, swc lowers it to ES5.
// `node build.mjs --watch` rebuilds on every change under src/ and public/.
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, readFileSync, rmSync, watch, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { build as esbuild } from "esbuild";
import { transform as swc } from "@swc/core";

const root = dirname(fileURLToPath(import.meta.url));
const dist = join(root, "dist");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));

const hash = (text) => createHash("sha1").update(text).digest("hex").slice(0, 8);

async function buildJs() {
  const bundled = await esbuild({
    entryPoints: [join(root, "src/main.ts")],
    bundle: true,
    write: false,
    format: "iife",
    target: "es2015",
    minify: true,
    legalComments: "none",
    define: { __DECK_VERSION__: JSON.stringify(pkg.version) },
    logLevel: "warning",
  });
  const out = await swc(bundled.outputFiles[0].text, {
    isModule: false,
    minify: true,
    jsc: {
      target: "es5",
      parser: { syntax: "ecmascript" },
      // ascii_only keeps the bundle safe if a host serves it without a charset.
      minify: { compress: true, mangle: true, format: { ascii_only: true } },
    },
  });
  return out.code;
}

async function buildCss() {
  const result = await esbuild({
    entryPoints: [join(root, "src/app.css")],
    bundle: true,
    write: false,
    minify: true,
    // Old WebViews: make esbuild lower what it can; the source is already prefixed by hand.
    target: ["chrome30", "safari9"],
    logLevel: "warning",
  });
  return result.outputFiles[0].text;
}

async function build() {
  const started = Date.now();
  mkdirSync(dist, { recursive: true });
  cpSync(join(root, "public"), dist, { recursive: true });

  const [js, css] = await Promise.all([buildJs(), buildCss()]);
  writeFileSync(join(dist, "app.js"), js);
  writeFileSync(join(dist, "app.css"), css);

  // Cache-bust by content so old WebViews never run a stale bundle.
  const html = readFileSync(join(root, "src/index.html"), "utf8")
    .replace("__JS_V__", hash(js))
    .replace("__CSS_V__", hash(css));
  writeFileSync(join(dist, "index.html"), html);

  const kb = (text) => (Buffer.byteLength(text) / 1024).toFixed(1);
  console.log(`deck built in ${Date.now() - started}ms (app.js ${kb(js)} KB, app.css ${kb(css)} KB)`);
}

async function safeBuild() {
  try {
    await build();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    if (!process.argv.includes("--watch")) process.exit(1);
  }
}

// Start from a clean dist; watch rebuilds overwrite in place so a running host never sees it empty.
rmSync(dist, { recursive: true, force: true });
await safeBuild();

if (process.argv.includes("--watch")) {
  let timer;
  const rebuild = () => {
    clearTimeout(timer);
    timer = setTimeout(safeBuild, 80);
  };
  for (const dir of ["src", "public"]) watch(join(root, dir), { recursive: true }, rebuild);
  console.log("watching src/ and public/");
}
