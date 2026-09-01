#!/usr/bin/env node

import { readFile, rename, stat, writeFile } from 'node:fs/promises';
import { basename, extname, resolve } from 'node:path';

const MAX_ATLAS_BYTES = 5 * 1024 * 1024;
const MAX_ARCHIVE_BYTES = 6 * 1024 * 1024;
const [manifestValue, atlasValue, outputValue] = process.argv.slice(2);

if (!manifestValue || !atlasValue || !outputValue) {
  fail('Usage: node scripts/package-dockskin.mjs <manifest.json> <atlas.png|webp> <output.dockskin>');
}

const manifestPath = resolve(manifestValue);
const atlasPath = resolve(atlasValue);
const outputPath = resolve(outputValue);
if (extname(outputPath).toLowerCase() !== '.dockskin') fail('Output must use the .dockskin extension');

const mimeType = atlasMimeType(atlasPath);
const atlasStats = await stat(atlasPath);
if (!atlasStats.isFile()) fail('Atlas path must point to a file');
if (atlasStats.size > MAX_ATLAS_BYTES) fail('Atlas exceeds the 5 MiB SDK limit');

const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
validateManifestEnvelope(manifest, mimeType);
const atlas = await readFile(atlasPath);
const archive = {
  format: 'dock.pet-skin',
  formatVersion: 1,
  manifest: {
    ...manifest,
    sprite: { mimeType }
  },
  atlasDataUrl: `data:${mimeType};base64,${atlas.toString('base64')}`
};
const serialized = `${JSON.stringify(archive)}\n`;
if (Buffer.byteLength(serialized) > MAX_ARCHIVE_BYTES) fail('Packaged skin exceeds the 6 MiB SDK limit');

const temporaryPath = `${outputPath}.tmp`;
await writeFile(temporaryPath, serialized, { encoding: 'utf8', mode: 0o600 });
await rename(temporaryPath, outputPath);
process.stdout.write(`Packaged ${basename(outputPath)} (${Buffer.byteLength(serialized)} bytes)\n`);

function atlasMimeType(path) {
  const extension = extname(path).toLowerCase();
  if (extension === '.png') return 'image/png';
  if (extension === '.webp') return 'image/webp';
  fail('Atlas must be a PNG or WebP image');
}

function validateManifestEnvelope(manifest, mimeType) {
  if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) fail('Manifest must be a JSON object');
  if (manifest.format !== 'dock.pet-skin' || manifest.formatVersion !== 1) {
    fail('Manifest must use dock.pet-skin format version 1');
  }
  if (manifest.spriteVersionNumber !== 2) fail('Manifest must use sprite version 2');
  if (typeof manifest.id !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(manifest.id)) {
    fail('Manifest id must be a stable lowercase identifier');
  }
  if (manifest.sprite?.mimeType && manifest.sprite.mimeType !== mimeType) {
    fail('Atlas MIME type does not match manifest.sprite.mimeType');
  }
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}
