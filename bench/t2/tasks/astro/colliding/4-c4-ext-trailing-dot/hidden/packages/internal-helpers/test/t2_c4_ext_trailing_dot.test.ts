import assert from 'node:assert/strict';
import { it } from 'node:test';
import { fileExtension } from '../src/path.ts';

it('gives no extension for a trailing dot', () => {
	assert.equal(fileExtension('file.'), '');
	assert.equal(fileExtension('/a/b.'), '');
});
