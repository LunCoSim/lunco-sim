import test from 'node:test';
import assert from 'node:assert/strict';
import { listSkills, readSkill } from '../src/skills.js';

test('the checkout MCP exposes the canonical skill catalogue', async () => {
  const skills = await listSkills();
  assert.ok(skills.length >= 38);
  assert.ok(skills.some((skill) => skill.name === 'luncosim-onboarding'));
  assert.ok(skills.some((skill) => skill.name === 'performance-profiling'));
});

test('a skill is readable by its validated name', async () => {
  const source = await readSkill('luncosim-onboarding');
  assert.match(source, /name: luncosim-onboarding/);
  await assert.rejects(() => readSkill('../AGENTS'), /Invalid skill name/);
});
