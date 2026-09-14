import { cp, mkdir, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const source = resolve(packageDir, '..', 'skills');
const destination = resolve(packageDir, 'skills');

await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(source, destination, { recursive: true });
console.error(`[LunCoSim MCP] Prepared ${destination} from canonical ${source}`);
