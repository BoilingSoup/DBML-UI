import { readFileSync } from 'node:fs';
import { Compiler, Filepath, MemoryProjectLayout } from '../upstream/dbml/packages/dbml-parse/src/index.js';
const layout = new MemoryProjectLayout();
const compiler = new Compiler(layout);
const fp = Filepath.from('/x');
layout.setSource(fp, readFileSync(process.argv[2] ?? '/tmp/probe4.dbml', 'utf8'));
const ast: any = compiler.parseFile(fp).getValue().ast;
const val = (n: any) => n?.expression?.variable?.value ?? n?.value ?? n?.op?.value ?? '';
const walk = (n: any, d: number) => {
  if (!n || d > 8) return;
  const kind = n.constructor?.name ?? typeof n;
  console.log('  '.repeat(d) + kind + ' ' + JSON.stringify(val(n)));
  for (const k of ['body','callee','args','elementList','leftExpression','rightExpression','expression']) {
    const v = (n as any)[k];
    if (Array.isArray(v)) { console.log('  '.repeat(d+1) + k + ':'); v.forEach((x: any) => walk(x, d+2)); }
    else if (v && typeof v === 'object') { console.log('  '.repeat(d+1) + k + ':'); walk(v, d+2); }
  }
};
const grp = ast.body[ast.body.length - 1];
walk(grp.body, 1);
