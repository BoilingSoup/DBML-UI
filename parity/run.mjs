//! The parity runner: feeds every corpus case to BOTH engines and diffs
//! the normalized diagnostics per parity/PARITY_SPEC.md.
//!
//!   node parity/run.mjs [case-name-filter]
//!
//! Gates: per case, the multiset of (type, text, row) must match; per
//! diagnostic, the quick-fix (shortTitle, newText) multiset must match.
//! Exits 1 on any divergence. Writes parity-report.json.

import { execFileSync, spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const CORPUS = process.env.CORPUS_DIR ?? join(ROOT, 'parity', 'corpus');
const OURS_BIN = join(ROOT, 'target', 'release', 'examples', 'parity_dump');
const FILTER = process.argv[2] ?? '';

const cases = readdirSync(CORPUS)
  .filter((f) => f.endsWith('.dbml') && f.includes(FILTER))
  .sort()
  .map((f) => ({ name: f.replace(/\.dbml$/, ''), path: join(CORPUS, f) }));
if (cases.length === 0) {
  console.error(`no corpus cases match "${FILTER}"`);
  process.exit(2);
}

// ---- our engine ----------------------------------------------------------
if (!existsSync(OURS_BIN)) {
  console.error('parity_dump not built — run: cargo build --release --example parity_dump');
  process.exit(2);
}
const oursRaw = JSON.parse(
  execFileSync(OURS_BIN, cases.map((c) => c.path), { encoding: 'utf8' }),
);
const ours = new Map(oursRaw.map((c) => [basename(c.path).replace(/\.dbml$/, ''), c]));

// ---- their engine (one tsx process for the whole corpus) ------------------
const casesFile = join(ROOT, 'parity', '.cases.json');
writeFileSync(casesFile, JSON.stringify(cases));
const theirsRun = spawnSync(
  'npx',
  ['tsx', 'theirs-dump-many.ts', casesFile],
  { cwd: join(ROOT, 'parity', 'driver'), encoding: 'utf8', timeout: 300000 },
);
if (theirsRun.status !== 0) {
  console.error('their engine failed:\n' + theirsRun.stdout + theirsRun.stderr);
  process.exit(2);
}
const theirs = new Map(
  JSON.parse(theirsRun.stdout).map((c) => [c.name, c]),
);

// ---- comparison -----------------------------------------------------------
// sort key: (row, type, text) — element-wise pairing after sorting makes
// multiplicity significant
const sortDiags = (list) =>
  [...list].sort(
    (a, b) => a.row - b.row || a.type.localeCompare(b.type) || a.text.localeCompare(b.text),
  );
const editsKey = (q) =>
  (q.edits ?? []).map((e) => `${e.start}-${e.end}:${e.newText}`).join(',');
const fixKey = (q) =>
  `${q.shortTitle}::${q.newText}::${q.title ?? ''}::${q.isPreferred ?? ''}::${editsKey(q)}`;
const fixMultiset = (list) => {
  list = list ?? [];
  const m = new Map();
  for (const q of list) m.set(fixKey(q), (m.get(fixKey(q)) ?? 0) + 1);
  return [...m.entries()].sort().join('|');
};
// explanation normalization: their string carries "- " bullet prefixes;
// ours is the bullet list. Both normalize to trimmed bullet lines.
const normExpl = (s) =>
  (s ?? '')
    .split('\n')
    .map((l) => l.trim().replace(/^- /, '').trim())
    .filter(Boolean)
    .join('\n');

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
  const od = sortDiags(o.diagnostics);
  const td = sortDiags(t.diagnostics);
  const problems = [];
  if (od.length !== td.length) {
    problems.push(`count: ours ${od.length} vs theirs ${td.length}`);
  }
  const n = Math.max(od.length, td.length);
  for (let i = 0; i < n; i++) {
    const a = od[i];
    const b = td[i];
    if (!a || !b) continue;
    if (a.type !== b.type) problems.push(`[${i}] type: ours ${a.type} vs theirs ${b.type}`);
    if (a.text !== b.text) problems.push(`[${i}] text: ours "${a.text}" vs theirs "${b.text}"`);
    if (a.row !== b.row) problems.push(`[${i}] row: ours ${a.row} vs theirs ${b.row}`);
    if ((a.category ?? null) !== (b.category ?? null)) {
      problems.push(`[${i}] category: ours "${a.category ?? null}" vs theirs "${b.category ?? null}"`);
    }
    if (normExpl(a.explanation) !== normExpl(b.explanation)) {
      problems.push(`[${i}] explanation: ours "${normExpl(a.explanation)}" vs theirs "${normExpl(b.explanation)}"`);
    }
    if (fixMultiset(a.quickFixes ?? []) !== fixMultiset(b.quickFixes ?? [])) {
      problems.push(`[${i}] quickFixes: ours [${(a.quickFixes ?? []).map(fixKey).join(', ')}] vs theirs [${(b.quickFixes ?? []).map(fixKey).join(', ')}]`);
    }
  }
  if (problems.length === 0) {
    pass += 1;
    report.push({ name, status: 'ok', count: od.length });
  } else {
    // classify: their NEW compiler (dbml-parse master) still has parser
    // gaps (inline refs, some operators) — cases where their side is
    // parse errors rather than semantic diagnostics are a different
    // class from genuine semantic divergences
    const parserGap = td.some(
      (x) =>
        x.type === 'error' &&
        (x.text.includes('A column must have a type') ||
          x.text.includes('expected identifier') ||
          x.text.includes('Inline column settings') ||
          x.text.includes('Invalid start of operand')),
    );
    fail += 1;
    report.push({
      name,
      status: 'divergent',
      class: parserGap ? 'their-parser-gap' : 'semantic',
      problems,
    });
  }
}

writeFileSync(
  join(ROOT, 'parity', 'parity-report.json'),
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
  if (r.status !== 'ok') {
    console.log(`DIVERGENT ${r.name}`);
    for (const p of r.problems ?? []) console.log(`   ${p}`);
  }
}
console.log(`\nparity: ${pass} pass, ${fail} divergent (report: parity/parity-report.json)`);
process.exit(fail > 0 ? 1 : 0);
