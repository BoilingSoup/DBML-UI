import { readFileSync } from 'node:fs';
import { Compiler, Filepath, MemoryProjectLayout } from '../upstream/dbml/packages/dbml-parse/src/index.js';
const layout = new MemoryProjectLayout();
const compiler = new Compiler(layout);
const fp = Filepath.from('/x');
const src = readFileSync(process.argv[2], 'utf8');
layout.setSource(fp, src);
const parsed: any = compiler.parseFile(fp).getValue();
console.log('--- tokens (kind [start..end] value) ---');
for (const t of parsed.tokens) {
  if (t.startPos.offset >= 330 && t.startPos.offset <= 560) {
    console.log(`${t.kind} [${t.startPos.offset}..${t.endPos.offset}] ${JSON.stringify(t.value)} invalid=${t.isInvalid}`);
  }
}
console.log('--- records element body rows ---');
const findRec = (n: any): any => {
  if (!n) return null;
  if (n.constructor?.name === 'ElementDeclarationNode' && (n.type?.value ?? '').toUpperCase() === 'RECORDS') return n;
  for (const k of ['body']) {
    const v = (n as any)[k];
    if (Array.isArray(v)) { for (const x of v) { const r = findRec(x); if (r) return r; } }
    else if (v && typeof v === 'object') { const r = findRec(v); if (r) return r; }
  }
  return null;
};
const rec = findRec(parsed.ast);
if (rec) {
  console.log('element span', rec.startPos?.offset, rec.endPos?.offset);
  const body = rec.body?.body ?? [];
  for (const row of body) {
    console.log(row.constructor.name, `[${row.startPos?.offset}..${row.endPos?.offset}]`,
      'callee', `[${row.callee?.startPos?.offset}..${row.callee?.endPos?.offset}]`,
      'args', (row.args ?? []).map((a: any) => `[${a.startPos?.offset}..${a.endPos?.offset}]${a.constructor.name}`).join(','));
  }
} else {
  console.log('no records element in AST');
}
