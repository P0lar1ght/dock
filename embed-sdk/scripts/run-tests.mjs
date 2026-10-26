// npm test：tsconfig.test.json 把源码连同 *.test.ts 编进 .test-dist/，再用 node --test 跑。
import { readdirSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const out = join(root, '.test-dist');
rmSync(out, { recursive: true, force: true });

const tsc = spawnSync('npx', ['tsc', '-p', 'tsconfig.test.json'], { cwd: root, stdio: 'inherit' });
if (tsc.status !== 0) process.exit(tsc.status ?? 1);

const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path);
    else if (name.endsWith('.test.js')) files.push(path);
  }
};
walk(out);
const run = spawnSync(process.execPath, ['--test', ...files], { cwd: root, stdio: 'inherit' });
process.exit(run.status ?? 1);
