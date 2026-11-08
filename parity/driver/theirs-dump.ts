//! Their-engine driver: read a .dbml file, run dbdiagram's open compiler,
//! dump every diagnostic with full fidelity as JSON on stdout.
import { readFileSync } from 'node:fs';
import {
  Compiler, DBMLDiagnosticsProvider, Filepath, MemoryProjectLayout,
} from '../upstream/dbml/packages/dbml-parse/src/index.js';

const [, , inputPath] = process.argv;
const content = readFileSync(inputPath, 'utf8');

const layout = new MemoryProjectLayout();
const fp = Filepath.from('/input.dbml');
layout.setSource(fp, content);

const compiler = new Compiler(layout);
const parseResult = compiler.parseFile(fp);
const parseValue = parseResult.getValue();

const provider = new DBMLDiagnosticsProvider(compiler);
const dump = (ds: any[]) => ds.map((d) => ({
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

const out = {
  parseOk: parseValue !== undefined,
  diagnostics: [
    ...dump(provider.provideErrors(fp) as any[]),
    ...dump(provider.provideWarnings(fp) as any[]),
    ...dump(provider.provideInfos(fp) as any[]),
  ],
};
console.log(JSON.stringify(out, null, 2));
