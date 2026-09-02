#!/usr/bin/env node
// Pass every argument through to the real binary.
import { spawn } from 'node:child_process';
import { binaryPath } from '../lib/index.js';

const child = spawn(binaryPath(), process.argv.slice(2), { stdio: 'inherit' });
child.on('error', (e) => {
  console.error(
    e.code === 'ENOENT'
      ? 'mar: binary not found. Reinstall the package, or set MAR_BINARY to a build of your own.'
      : `mar: ${e.message}`,
  );
  process.exit(127);
});
child.on('close', (code, signal) => process.exit(signal ? 128 : (code ?? 0)));
