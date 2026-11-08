//! The schema-parity runner: feeds every corpus case to BOTH engines'
//! schema output and diffs the normalized Database JSON per
//! parity/PARITY_SCHEMA_SPEC.md.
//!
//!   node parity/run_schema.mjs [case-name-filter]
//!
//! Gates: per case, both databases are null, or both non-null and deep-
//! equal after canonicalization (object keys sorted). Divergences report
//! the JSON path of each mismatch. Writes parity-schema-report.json.

import { execFileSync, spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const CORPUS = process.env.CORPUS_DIR ?? join(ROOT, 'parity', 'corpus');
const OURS_BIN = join(ROOT, 'target', 'release', 'examples', 'parity_schema');
const FILTER = process.argv[2] ?? '';

const cases = readdirSync(CORPUS)
  .filter((f) => f.endsWith('.dbml') && f.includes(FILTER))
  .sort()
  .map((f) => ({ name: f.replace(/\.dbml$/, ''), path: join(CORPUS, f) }));
if (cases.length === 0) {
  console.error(`no corpus cases match "${FILTER}"`);
  process.exit(2);
}

if (!existsSync(OURS_BIN)) {
  console.error('parity_schema not built — run: cargo build --release --example parity_schema');
  process.exit(2);
}
const oursRaw = JSON.parse(
  execFileSync(OURS_BIN, cases.map((c) => c.path), { encoding: 'utf8' }),
);
const ours = new Map(oursRaw.map((c) => [basename(c.path).replace(/\.dbml$/, ''), c]));

const casesFile = join(ROOT, 'parity', '.cases-schema.json');
writeFileSync(casesFile, JSON.stringify(cases));
const theirsRun = spawnSync(
  'npx',
  ['tsx', 'theirs-schema-many.ts', casesFile],
  { cwd: join(ROOT, 'parity', 'driver'), encoding: 'utf8', timeout: 300000 },
);
if (theirsRun.status !== 0) {
  console.error('their engine failed:\n' + theirsRun.stdout + theirsRun.stderr);
  process.exit(2);
}
const theirs = new Map(
  JSON.parse(theirsRun.stdout).map((c) => [c.name, c]),
);

// canonicalize: recursively sort object keys so key order never matters
const canon = (v) => {
  if (Array.isArray(v)) return v.map(canon);
  if (v && typeof v === 'object') {
    const out = {};
    for (const k of Object.keys(v).sort()) out[k] = canon(v[k]);
    return out;
  }
  return v;
};

// deep diff with path reporting
const diff = (a, b, path, out) => {
  if (a === b) return;
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) out.push(`${path}: length ${a.length} vs ${b.length}`);
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      diff(a[i], b[i], `${path}[${i}]`, out);
    }
    return;
  }
  if (a && b && typeof a === 'object' && typeof b === 'object') {
    for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
      diff(a[k], b[k], `${path}.${k}`, out);
    }
    return;
  }
  out.push(`${path}: ours ${JSON.stringify(a)} vs theirs ${JSON.stringify(b)}`);
};

const report = [];
let pass = 0;
let fail = 0;
for (const { name } of cases) {
  const o = ours.get(name);
  const t = theirs.get(name);
  if (!o || !t) {
    report.push({ name, status: 'missing-engine-output' });
    fail += 1;
    continue;
  }
  const od = canon(o.database);
  const td = canon(t.database);
  if (od === null && td === null) {
    pass += 1;
    report.push({ name, status: 'ok-both-null' });
    continue;
  }
  if (od === null || td === null) {
    fail += 1;
    report.push({
      name,
      status: 'divergent',
      problems: [
        od === null
          ? 'ours null (blocked by our errors) while theirs produced a database'
          : 'theirs null while ours produced a database',
        ...(od === null ? o.errors.map((e) => `our parse error: ${e}`) : []),
        ...(td === null ? (t.interpretErrors ?? []).map((e) => `their error: ${e.message}`) : []),
      ],
    });
    continue;
  }
  const problems = [];
  diff(od, td, '', problems);
  if (problems.length === 0) {
    pass += 1;
    report.push({ name, status: 'ok' });
  } else {
    fail += 1;
    report.push({ name, status: 'divergent', problems: problems.slice(0, 12) });
  }
}

writeFileSync(
  join(ROOT, 'parity', 'parity-schema-report.json'),
  JSON.stringify(
    {
      pinnedCommit: readFileSync(join(ROOT, 'parity', 'upstream-pin.txt'), 'utf8').trim(),
      pass,
      fail,
      cases: report,
    },
    null,
    2,
  ),
);

for (const r of report) {
  if (r.status === 'divergent' || r.status === 'missing-engine-output') {
    console.log(`DIVERGENT ${r.name}`);
    for (const p of r.problems ?? []) console.log(`   ${p}`);
  }
}
console.log(`\nschema parity: ${pass} pass, ${fail} divergent (report: parity/parity-schema-report.json)`);
process.exit(fail > 0 ? 1 : 0);
