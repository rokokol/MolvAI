// SPDX-License-Identifier: MIT
// Растеризация иконки MolvAI без внешних зависимостей: только zlib из стандартной
// библиотеки Node. Рисуем ту же форму, что в molva.svg (микрофон на скруглённом
// квадрате), со сглаживанием 3x3 суперсэмплингом.
//
// Запуск: node crates/molva-gui/icons/gen-icons.mjs
// Результат: icon.png (1024, исходник для `cargo tauri icon`) и три иконки трея.

import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/** CRC32 для чанков PNG. */
const crcTable = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

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

/** RGBA-пиксели (Uint8Array размера size*size*4) в буфер PNG. */
function encodePng(size, rgba) {
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y += 1) {
    raw[y * (stride + 1)] = 0; // фильтр None
    Buffer.from(rgba.buffer, y * stride, stride).copy(raw, y * (stride + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // бит на канал
  ihdr[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// --- Геометрия в единичных координатах (0..1), совпадает с molva.svg ---

const clamp01 = (v) => Math.min(1, Math.max(0, v));

function insideRoundedRect(x, y, x0, y0, x1, y1, r) {
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  if (x < x0 || x > x1 || y < y0 || y > y1) return false;
  const dx = x - cx;
  const dy = y - cy;
  return dx * dx + dy * dy <= r * r;
}

/** Микрофон: капсула, дуга-подставка, ножка и основание. */
function insideMic(x, y) {
  if (insideRoundedRect(x, y, 0.4, 0.2, 0.6, 0.6, 0.1)) return true;
  if (y >= 0.5) {
    const dx = x - 0.5;
    const dy = y - 0.5;
    const d = Math.sqrt(dx * dx + dy * dy);
    if (d >= 0.19 && d <= 0.26) return true;
  }
  if (x >= 0.465 && x <= 0.535 && y >= 0.755 && y <= 0.85) return true;
  if (insideRoundedRect(x, y, 0.35, 0.83, 0.65, 0.9, 0.032)) return true;
  return false;
}

/**
 * Рисует иконку.
 * `bg` — цвет подложки [r,g,b] или null (прозрачный фон, для трея),
 * `fg` — цвет микрофона.
 */
function render(size, bg, fg) {
  const rgba = new Uint8Array(size * size * 4);
  const samples = 3;
  for (let py = 0; py < size; py += 1) {
    for (let px = 0; px < size; px += 1) {
      let bgHits = 0;
      let fgHits = 0;
      for (let sy = 0; sy < samples; sy += 1) {
        for (let sx = 0; sx < samples; sx += 1) {
          const x = (px + (sx + 0.5) / samples) / size;
          const y = (py + (sy + 0.5) / samples) / size;
          if (bg && insideRoundedRect(x, y, 0.02, 0.02, 0.98, 0.98, 0.215)) bgHits += 1;
          if (insideMic(x, y)) fgHits += 1;
        }
      }
      const total = samples * samples;
      const bgA = bgHits / total;
      const fgA = fgHits / total;
      // Микрофон поверх подложки: сначала подложка, потом передний план.
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      if (bg) {
        r = bg[0];
        g = bg[1];
        b = bg[2];
        a = bgA;
      }
      if (fgA > 0) {
        const na = fgA + a * (1 - fgA);
        r = na === 0 ? 0 : (fg[0] * fgA + r * a * (1 - fgA)) / na;
        g = na === 0 ? 0 : (fg[1] * fgA + g * a * (1 - fgA)) / na;
        b = na === 0 ? 0 : (fg[2] * fgA + b * a * (1 - fgA)) / na;
        a = na;
      }
      const o = (py * size + px) * 4;
      rgba[o] = Math.round(clamp01(r / 255) * 255);
      rgba[o + 1] = Math.round(clamp01(g / 255) * 255);
      rgba[o + 2] = Math.round(clamp01(b / 255) * 255);
      rgba[o + 3] = Math.round(clamp01(a) * 255);
    }
  }
  return rgba;
}

const brand = [59, 91, 219];
const white = [255, 255, 255];
const outputs = [
  ["icon.png", 1024, brand, white],
  // Трей: прозрачный фон, цвет микрофона кодирует состояние демона.
  ["tray-idle.png", 64, null, [170, 178, 195]],
  ["tray-recording.png", 64, null, [230, 70, 70]],
  ["tray-processing.png", 64, null, [240, 170, 40]],
];

for (const [name, size, bg, fg] of outputs) {
  const png = encodePng(size, render(size, bg, fg));
  writeFileSync(join(here, name), png);
  console.log(`${name}: ${size}x${size}, ${png.length} байт`);
}
