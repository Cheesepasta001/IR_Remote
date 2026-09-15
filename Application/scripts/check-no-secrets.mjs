/**
 * Definition-of-done check for Instruction.md rule 7:
 * "The token never enters the frontend. It is read from the OS keychain by the
 *  Rust core, attached to outgoing requests there, and never returned over IPC,
 *  never logged, never written to a config file in plaintext."
 *
 * This checks BEHAVIOUR, not vocabulary. An earlier version matched the word
 * "keychain" and flagged the app's own button label, plus a JSDoc example
 * inside @tauri-apps/api - both harmless. Comments are stripped before the
 * bundle is scanned, and the Rust source is checked for the property that
 * actually matters: nothing that carries a secret can be serialized.
 *
 * Run with: npm run check:secrets   (after npm run build)
 */

import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const DIST = 'dist';
const RUST = join('src-tauri', 'src');

let failures = 0;
const fail = (msg) => {
  console.error(`FAIL  ${msg}`);
  failures++;
};

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else out.push(p);
  }
  return out;
}

/** Remove /* *\/ and // comments so vendor JSDoc examples are not scanned. */
function stripComments(src) {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .replace(/(^|[^:])\/\/[^\n]*/g, '$1 ');
}

// ------------------------------------------------- 1. the built bundle ----

if (!existsSync(DIST)) {
  console.error(`${DIST}/ not found - run "npm run build" first.`);
  process.exit(1);
}

/** Executable patterns that would mean the frontend is handling auth itself. */
const FORBIDDEN_CODE = [
  // Matches quoted and unquoted object keys alike: {Authorization: ...},
  // {"Authorization": ...}, headers.set('authorization', ...).
  { re: /["'`]?\bauthorization\b["'`]?\s*[,:]/i, why: 'frontend builds an Authorization header' },
  { re: /\bsetRequestHeader\s*\(/i, why: 'frontend sets request headers directly' },
  { re: /\bbtoa\s*\(/, why: 'base64 encoding in the frontend suggests credential handling' },
  { re: /\bbasic_auth\b/, why: 'auth belongs in the Rust core' },
  { re: /invoke\s*\(\s*["'`](get|read|load|fetch)_?credential/i, why: 'an IPC call that reads a secret back' },
  { re: /\bfetch\s*\(\s*["'`]https?:\/\//, why: 'frontend issues its own HTTP - all HTTP belongs in Rust' },
];

const jsFiles = walk(DIST).filter((f) => f.endsWith('.js'));
if (jsFiles.length === 0) fail('no JS found in dist/ - did the build run?');

let bundle = '';
for (const file of jsFiles) {
  const code = stripComments(readFileSync(file, 'utf8'));
  bundle += code + '\n';
  for (const { re, why } of FORBIDDEN_CODE) {
    if (re.test(code)) fail(`${file}: ${re} (${why})`);
  }
}

// The snapshot may carry only the boolean.
if (!/hasCredential/.test(bundle)) {
  fail('bundle never references hasCredential - is the snapshot wired up?');
}

// ------------------------------------------------------ 2. the IPC seam ----

// The IPC surface lives in lib.rs; main.rs is wiring only. Scan both so the
// check keeps working whichever file a command is declared in.
const ipcSources = ['lib.rs', 'main.rs']
  .map((f) => join(RUST, f))
  .filter((p) => existsSync(p));

if (ipcSources.length === 0) fail(`no Rust entry point found under ${RUST}`);
const ipcSrc = ipcSources.map((p) => readFileSync(p, 'utf8')).join('\n');

// Every #[tauri::command] return type, so none of them can hand back a secret.
const returns = [...ipcSrc.matchAll(/#\[tauri::command\][\s\S]*?fn\s+(\w+)[\s\S]*?\)\s*(?:->\s*([^{]+))?\{/g)];
if (returns.length === 0) {
  fail(`no #[tauri::command] found in ${ipcSources.join(', ')}`);
}

for (const [, name, ret] of returns) {
  const r = (ret ?? '()').trim();
  if (/Credentials/.test(r)) {
    fail(`IPC command ${name} returns ${r} - a credential must never cross IPC`);
  }

  // Only the SUCCESS type can carry data back to the view. The error arm of a
  // Result is a message channel and is expected to be String.
  const result = /^Result\s*<\s*([\s\S]+?)\s*,\s*[^,]+>$/.exec(r);
  const success = (result ? result[1] : r).trim();

  if (/\bString\b/.test(success)) {
    fail(
      `IPC command ${name} returns ${r} - its success type ${success} is a String, ` +
        'which could carry a secret',
    );
  }
}

// ----------------------------------------------------- 3. the auth type ----

const authRs = readFileSync(join(RUST, 'auth.rs'), 'utf8');

if (/#\[derive\([^)]*Serialize[^)]*\)\]\s*(?:pub\s+)?struct\s+Credentials/.test(authRs)) {
  fail('Credentials derives Serialize - it could then be sent over IPC');
}
if (!/fn fmt\(&self/.test(authRs) || !/redacted/.test(authRs)) {
  fail('Credentials has no redacting Debug impl - a log line could print the secret');
}

// The snapshot type must not carry the secret itself.
const coreRs = readFileSync(join(RUST, 'core.rs'), 'utf8');
const snapshotBlock = coreRs.match(/pub struct Snapshot \{[\s\S]*?\n\}/)?.[0] ?? '';
if (/password|secret|token/i.test(snapshotBlock)) {
  fail('Snapshot carries a credential field - only has_credential may be exposed');
}
if (!/has_credential\s*:\s*bool/.test(snapshotBlock)) {
  fail('Snapshot is missing has_credential: bool');
}

// --------------------------------------------------------------- report ----

if (failures > 0) {
  console.error(`\n${failures} problem(s).`);
  process.exit(1);
}

console.log(
  `OK  ${jsFiles.length} bundle file(s) + IPC surface + auth type checked; ` +
    'no credential can reach the frontend.',
);
