#!/usr/bin/env node
// Copies the browser libraries from node_modules (installed by `npm ci` from package-lock.json) to
// where the site serves them. The copies are not committed; run this after `npm ci`, i.e. once after
// cloning and again after a dependency update (Dependabot PR).
//
//   npm ci && npm run vendor              → into public/
//   node scripts/vendor.mjs <dir>         → into <dir> (the Docker build)
//
// A file a package no longer ships stops the script, so a breaking update can't go unnoticed.

import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const modules = join(root, 'node_modules');
const out = resolve(process.argv[2] || join(root, 'public'));

// [file in node_modules, path below the output directory]
const FILES = [
  ['jszip/dist/jszip.min.js', 's/js/jszip.min.js'],
  // marked ≥ 16 ships its browser build (minified UMD) only as lib/marked.umd.js
  ['marked/lib/marked.umd.js', 's/js/marked.min.js'],
  ['@cap.js/widget/cap.min.js', 's/cap/cap.min.js'],
  ['@cap.js/widget/LICENSE', 's/cap/LICENSE'],
  ['@cap.js/wasm/browser/cap_wasm_bg.wasm', 's/cap/cap_wasm_bg.wasm'],
];

// Ruffle (Flash emulator): everything the browser loads, plus licenses; no source maps
const RUFFLE = { from: '@ruffle-rs/ruffle', to: 's/ruffle', keep: (f) => /\.(js|wasm)$/.test(f) || /^LICENSE_/.test(f) || f === 'package.json' };

function fail(msg) {
  console.error(`vendor: ${msg}`);
  process.exit(1);
}

if (!existsSync(modules)) fail('node_modules missing, run `npm ci` first');

for (const [from, to] of FILES) {
  const src = join(modules, from);
  if (!existsSync(src)) fail(`${from} not found (did the package change its layout?)`);
  const dest = join(out, to);
  mkdirSync(dirname(dest), { recursive: true });
  copyFileSync(src, dest);
  console.log(`${from} -> ${to}`);
}

const ruffleSrc = join(modules, RUFFLE.from);
if (!existsSync(ruffleSrc)) fail(`${RUFFLE.from} not found`);
const ruffleFiles = readdirSync(ruffleSrc).filter(RUFFLE.keep);
if (!ruffleFiles.includes('ruffle.js')) fail(`${RUFFLE.from}/ruffle.js not found (did the package change its layout?)`);
const ruffleDest = join(out, RUFFLE.to);
// Replaced as a whole: chunk file names change with every release
rmSync(ruffleDest, { recursive: true, force: true });
mkdirSync(ruffleDest, { recursive: true });
for (const f of ruffleFiles) copyFileSync(join(ruffleSrc, f), join(ruffleDest, f));
console.log(`${RUFFLE.from} -> ${RUFFLE.to} (${ruffleFiles.length} files)`);
