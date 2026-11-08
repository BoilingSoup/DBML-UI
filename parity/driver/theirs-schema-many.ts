//! Batch driver: interprets every corpus case and dumps the normalized
//! `Database` JSON (parity/PARITY_SCHEMA_SPEC.md). Positions are dropped
//! in v1 — structural parity first, token anchors later.
//!
//!   npx tsx theirs-schema-many.ts <cases.json>
import { readFileSync } from 'node:fs';
import {
  Compiler, Filepath, MemoryProjectLayout,
} from '../upstream/dbml/packages/dbml-parse/src/index.js';

const cases: { name: string; path: string }[] = JSON.parse(readFileSync(process.argv[2], 'utf8'));

// v2: token positions join the contract as OFFSETS ONLY (their Position
// is {offset, line, column} over UTF-16 — only the offset is
// language-neutral). filepath drops; line/column drop with it.
const strip = (v: any): any => {
  if (Array.isArray(v)) return v.map(strip);
  if (v && typeof v === 'object') {
    const out: any = {};
    for (const [k, val] of Object.entries(v)) {
      if (k === 'filepath') continue;
      if (k === 'token' && val && typeof val === 'object' && 'start' in val) {
        out[k] = { start: { offset: val.start.offset }, end: { offset: val.end.offset } };
        continue;
      }
      out[k] = strip(val);
    }
    return out;
  }
  return v;
};

const layout = new MemoryProjectLayout();
const compiler = new Compiler(layout);
const out: any[] = [];
for (const c of cases) {
  const content = readFileSync(c.path, 'utf8');
  const fp = Filepath.from(`/corpus/${c.name}`);
  layout.setSource(fp, content);
  let database: any = null;
  let interpretErrors: any[] = [];
  try {
    const report = (compiler as any).interpretFile(fp);
    const errs = report.getErrors?.() ?? [];
    interpretErrors = errs.map((e: any) => ({ code: e.code, message: e.message }));
    database = strip(report.getValue() ?? null);
  } catch (e: any) {
    interpretErrors = [{ code: 'THREW', message: String(e?.message ?? e) }];
  }
  out.push({ name: c.name, database, interpretErrors });
}
console.log(JSON.stringify(out, null, 2));
