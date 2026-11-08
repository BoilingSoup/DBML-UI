//! Batch driver: reads a JSON array of {name, path} on argv[1-side file],
//! dumps {name, diagnostics} for each — one process for the whole corpus.
import { readFileSync } from 'node:fs';
import {
  Compiler, DBMLDiagnosticsProvider, Filepath, MemoryProjectLayout,
} from '../upstream/dbml/packages/dbml-parse/src/index.js';

const cases: { name: string; path: string }[] = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const dump = (ds: any[]) => ds.map((d: any) => ({
  type: d.type,
  text: d.text,
  startRow: d.startRow,
  startColumn: d.startColumn,
  endRow: d.endRow,
  endColumn: d.endColumn,
  category: d.category ?? null,
  explanation: d.explanation ?? null,
  quickFixes: (d.quickFixes ?? []).map((q: any) => ({
    title: q.title,
    shortTitle: q.shortTitle ?? null,
    isPreferred: q.isPreferred ?? null,
    edits: q.edits,
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
