//! Batch driver: reads a JSON array of {name, path} on argv[1-side file],
//! dumps {name, diagnostics} for each — one process for the whole corpus.
import { readFileSync } from 'node:fs';
import {
  Compiler, DBMLDiagnosticsProvider, Filepath, MemoryProjectLayout,
} from '../upstream/dbml/packages/dbml-parse/src/index.js';

const cases: { name: string; path: string }[] = JSON.parse(readFileSync(process.argv[2], 'utf8'));
// the contract shape (parity/PARITY_SPEC.md): {type, text, row,
// quickFixes: [{shortTitle, newText}]} — their row/col spans and
// multi-edit fixes normalize to row + the joined edit text
const dump = (ds: any[]) => ds.map((d: any) => ({
  type: d.type,
  text: d.text,
  row: d.startRow,
  column: d.startColumn ?? null,
  endRow: d.endRow ?? null,
  endColumn: d.endColumn ?? null,
  category: d.category ?? null,
  explanation: d.explanation ?? null,
  code: d.code ?? null,
  quickFixes: (d.quickFixes ?? []).map((q: any) => ({
    title: q.title ?? null,
    shortTitle: q.shortTitle ?? null,
    isPreferred: q.isPreferred ?? null,
    newText: (q.edits ?? []).map((e: any) => e.newText).join(' | '),
    edits: (q.edits ?? []).map((e: any) => ({ start: e.start, end: e.end, newText: e.newText })),
  })),
}));
const layout = new MemoryProjectLayout();
const compiler = new Compiler(layout);
const provider = new DBMLDiagnosticsProvider(compiler);
const out: any[] = [];
for (const c of cases) {
  const content = readFileSync(c.path, 'utf8');
  const fp = Filepath.from(`/corpus/${c.name}`);
  layout.setSource(fp, content);
  const parse = compiler.parseFile(fp).getValue();
  const diags = [
    ...dump(provider.provideErrors(fp) as any[]),
    ...dump(provider.provideWarnings(fp) as any[]),
    ...dump(provider.provideInfos(fp) as any[]),
  ];
  out.push({ name: c.name, parseOk: parse !== undefined, diagnostics: diags });
}
console.log(JSON.stringify(out, null, 2));
