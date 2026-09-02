#!/usr/bin/env node
// Fetch the binary for this platform from the matching GitHub release.
//
// The alternative, publishing one npm package per platform as optional
// dependencies, means seven packages to keep in step with every release. A
// download keeps the source of truth in one place: the GitHub release.
import { createWriteStream, existsSync, mkdirSync, chmodSync, renameSync } from 'node:fs';
import { pipeline } from 'node:stream/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Extract } from 'node:zlib';

const require = createRequire(import.meta.url);
const { version, repository } = require('../package.json');
const here = dirname(fileURLToPath(import.meta.url));
const binDir = join(here, '..', 'bin');

const TARGETS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

async function main() {
  // A local build wins: contributors run from source and must not have it
  // silently replaced by a published binary.
  const local = join(here, '..', '..', 'target', 'release', binaryName());
  if (existsSync(local)) {
    console.log(`mini-agent-reader: using the local build at ${local}`);
    return;
  }

  const key = `${process.platform}-${process.arch}`;
  const target = TARGETS[key];
  if (!target) {
    console.warn(
      `mini-agent-reader: no prebuilt binary for ${key}. ` +
      `Build from source with "cargo build --release" and set MAR_BINARY to the result.`,
    );
    return;
  }

  const repo = repository.url.replace(/^git\+/, '').replace(/\.git$/, '');
  const asset = `mar-${target}.tar.gz`;
  const url = `${repo}/releases/download/v${version}/${asset}`;

  mkdirSync(binDir, { recursive: true });
  const archive = join(binDir, asset);

  console.log(`mini-agent-reader: downloading ${asset}`);
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok) {
    console.warn(
      `mini-agent-reader: could not download ${url} (${response.status}). ` +
      `Set MAR_BINARY to a "mar" you built yourself, or install from source.`,
    );
    return;
  }
  await pipeline(response.body, createWriteStream(archive));

  // tar is present on every platform Node 18 supports, Windows included.
  const { execFileSync } = await import('node:child_process');
  execFileSync('tar', ['-xzf', archive, '-C', binDir], { stdio: 'inherit' });

  const binary = join(binDir, binaryName());
  if (existsSync(binary) && process.platform !== 'win32') chmodSync(binary, 0o755);
  console.log(`mini-agent-reader: installed ${binary}`);
}

function binaryName() {
  return process.platform === 'win32' ? 'mar.exe' : 'mar';
}

main().catch((e) => {
  // A failed download must not fail the install: the caller may intend to
  // point MAR_BINARY at their own build.
  console.warn(`mini-agent-reader: ${e.message}`);
});
