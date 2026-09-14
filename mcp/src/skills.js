import { access, readdir, readFile } from 'node:fs/promises';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const PACKAGE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SKILL_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

async function existingDirectory(path) {
  try {
    await access(path);
    return path;
  } catch {
    return null;
  }
}

/**
 * Use the packaged bundle when installed; use the repository source when the
 * checkout MCP configuration runs the server. This is a read-only lookup and
 * never makes the installed server depend on a source-checkout path.
 */
export async function findSkillsRoot() {
  return (await existingDirectory(join(PACKAGE_ROOT, 'skills')))
    ?? (await existingDirectory(resolve(PACKAGE_ROOT, '..', 'skills')));
}

function frontmatterValue(source, field) {
  const lines = source.split('\n');
  const start = lines[0]?.trim() === '---' ? 1 : 0;
  let value = '';
  for (let index = start; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() === '---') break;
    const fieldMatch = line.match(new RegExp(`^${field}:\\s*(.*)$`));
    if (fieldMatch) {
      value = fieldMatch[1].trim();
      if (value === '>' || value === '|') {
        const continuation = [];
        for (let next = index + 1; next < lines.length; next += 1) {
          if (!/^\s+/.test(lines[next])) break;
          continuation.push(lines[next].trim());
        }
        value = continuation.join(' ');
      }
      break;
    }
  }
  return value.replace(/^['"]|['"]$/g, '');
}

export async function listSkills() {
  const root = await findSkillsRoot();
  if (!root) return [];
  const entries = await readdir(root, { withFileTypes: true });
  const skills = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || !SKILL_NAME.test(entry.name)) continue;
    const path = join(root, entry.name, 'SKILL.md');
    const source = await readFile(path, 'utf8');
    const name = frontmatterValue(source, 'name');
    const description = frontmatterValue(source, 'description');
    if (name !== entry.name || !description) {
      throw new Error(`Invalid skill frontmatter: ${entry.name}`);
    }
    skills.push({ name, description, uri: `lunco://skills/${name}` });
  }
  return skills.sort((left, right) => left.name.localeCompare(right.name));
}

export async function readSkill(name) {
  if (typeof name !== 'string' || !SKILL_NAME.test(name)) {
    throw new Error(`Invalid skill name: ${name}`);
  }
  const root = await findSkillsRoot();
  if (!root) throw new Error('LunCoSim skill bundle is not installed');
  const path = resolve(root, name, 'SKILL.md');
  const rootPath = resolve(root);
  if (!path.startsWith(`${rootPath}${sep}`)) {
    throw new Error(`Invalid skill path: ${name}`);
  }
  return readFile(path, 'utf8');
}
