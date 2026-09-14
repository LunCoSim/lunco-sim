import { cp, mkdir, rm } from 'node:fs/promises';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const source = resolve(packageDir, '..', 'skills');
const destination = resolve(packageDir, 'skills');

await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(source, destination, {
  recursive: true,
  filter: (path) => !path.split(sep).includes('__pycache__') && !path.endsWith('.pyc'),
});
console.error(`[LunCoSim MCP] Prepared ${destination} from canonical ${source}`);
