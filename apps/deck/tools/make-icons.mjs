// Rasterizes the OmniPanel logo into the PNG icons used by the manifest, iOS and old browsers.
// Run `node tools/make-icons.mjs` after changing the logo; the PNGs are committed.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const out = join(dirname(fileURLToPath(import.meta.url)), "../public/icons");
mkdirSync(out, { recursive: true });

// Shapes live in the logo's 1024x1024 space. `ring` draws a stroke instead of a fill.
const rect = (x, y, size, r, fill) => ({ x, y, w: size, h: size, r, fill });
const ring = (x, y, size, r, width, fill) => ({ x, y, w: size, h: size, r, fill, ring: width });

const tiles = [];
for (const [col, row] of [[0, 0], [1, 0], [2, 0], [0, 1], [2, 1], [0, 2], [1, 2], [2, 2]]) {
  tiles.push(rect(164 + col * 252, 164 + row * 252, 192, 44, "#1d1d1d"));
}
const center = rect(416, 416, 192, 44, "#4f8cff");

// The logo as shipped: rounded black plate on a transparent canvas.
const plate = [rect(64, 64, 896, 208, "#000000"), ring(68, 68, 888, 204, 8, "#262626"), ...tiles, center];
// Full-bleed variant (iOS and maskable icons apply their own mask): tile grid only, scaled down.
const bleed = [rect(-2048, -2048, 5120, 0, "#000000"),...tiles, center];

const hex = (c) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));

function insideRounded(px, py, s) {
  const cx = Math.min(Math.max(px, s.x + s.r), s.x + s.w - s.r);
  const cy = Math.min(Math.max(py, s.y + s.r), s.y + s.h - s.r);
  const dx = px - cx;
  const dy = py - cy;
  return px >= s.x && px <= s.x + s.w && py >= s.y && py <= s.y + s.h && dx * dx + dy * dy <= s.r * s.r + 1e-9;
}

function covers(px, py, s) {
  if (!s.ring) return insideRounded(px, py, s);
  const inner = { x: s.x + s.ring, y: s.y + s.ring, w: s.w - 2 * s.ring, h: s.h - 2 * s.ring, r: Math.max(s.r - s.ring, 0) };
  return insideRounded(px, py, s) && !insideRounded(px, py, inner);
}

/** Renders shapes into RGBA with 4x4 supersampling. `scale` shrinks the artwork around the center. */
function render(size, shapes, scale = 1) {
  const buf = Buffer.alloc(size * size * 4);
  const ss = 4;
  const colors = shapes.map((s) => hex(s.fill));
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          // Pixel sample -> logo space, undoing the scale around the center.
          const lx = ((x + (sx + 0.5) / ss) / size - 0.5) * 1024 / scale + 512;
          const ly = ((y + (sy + 0.5) / ss) / size - 0.5) * 1024 / scale + 512;
          for (let i = shapes.length - 1; i >= 0; i--) {
            if (covers(lx, ly, shapes[i])) {
              r += colors[i][0]; g += colors[i][1]; b += colors[i][2]; a += 255;
              break;
            }
          }
        }
      }
      const n = ss * ss;
      const o = (y * size + x) * 4;
      // Premultiplied average -> straight alpha.
      buf[o + 3] = Math.round(a / n);
      if (a > 0) {
        buf[o] = Math.round(r / (a / 255));
        buf[o + 1] = Math.round(g / (a / 255));
        buf[o + 2] = Math.round(b / (a / 255));
      }
    }
  }
  return buf;
}

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
}
function png(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const rows = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) rgba.copy(rows, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(rows, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const files = [
  ["favicon-32.png", 32, plate, 1],
  ["icon-192.png", 192, plate, 1],
  ["icon-512.png", 512, plate, 1],
  ["icon-maskable-512.png", 512, bleed, 0.8],
  ["apple-touch-icon.png", 180, bleed, 0.9],
];
for (const [name, size, shapes, scale] of files) {
  writeFileSync(join(out, name), png(size, render(size, shapes, scale)));
  console.log("wrote", name);
}
