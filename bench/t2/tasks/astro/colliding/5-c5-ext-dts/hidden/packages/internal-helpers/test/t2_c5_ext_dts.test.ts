import assert from 'node:assert/strict';
import { it } from 'node:test';
import { fileExtension } from '../src/path.ts';

it('keeps .d.ts whole', () => {
	assert.equal(fileExtension('/types/env.d.ts'), '.d.ts');
	assert.equal(fileExtension('/a/b.ts'), '.ts');
});
