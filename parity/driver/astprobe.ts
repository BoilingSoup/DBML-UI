import { readFileSync } from 'node:fs';
import { Compiler, Filepath, MemoryProjectLayout, ElementDeclarationNode } from '../upstream/dbml/packages/dbml-parse/src/index.js';
const layout = new MemoryProjectLayout();
const compiler = new Compiler(layout);
const fp = Filepath.from('/x');
const src = readFileSync(process.argv[2], 'utf8');
layout.setSource(fp, src);
const ast: any = compiler.parseFile(fp).getValue().ast;
const walk = (n: any, depth: number) => {
  if (!n || depth > 12) return;
  const kind = n.constructor?.name ?? typeof n;
  const val = n.value ?? n.op?.value ?? '';
  console.log('  '.repeat(depth) + kind + ' ' + JSON.stringify(val).slice(0, 24));
  for (const k of ['body', 'callee', 'args', 'leftExpression', 'rightExpression', 'expression', 'argumentList', 'elementList']) {
    const v = (n as any)[k];
    if (Array.isArray(v)) { console.log('  '.repeat(depth+1) + k + ':'); v.forEach((x: any) => walk(x, depth+2)); }
    else if (v && typeof v === 'object') { console.log('  '.repeat(depth+1) + k + ':'); walk(v, depth+2); }
  }
};
walk(ast, 0);
