import assert from 'node:assert/strict';
import { it } from 'node:test';
import { validatePackageName } from '../src/cli.ts';

it('refuses names longer than 214 characters', () => {
	assert.equal(validatePackageName('a'.repeat(215)), false);
	assert.equal(validatePackageName('a'.repeat(214)), true);
	assert.equal(validatePackageName('react'), true);
});
