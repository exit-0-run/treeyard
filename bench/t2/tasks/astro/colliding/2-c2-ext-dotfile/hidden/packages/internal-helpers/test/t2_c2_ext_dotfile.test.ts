import assert from 'node:assert/strict';
import { it } from 'node:test';
import { fileExtension } from '../src/path.ts';

it('gives no extension to a dotfile', () => {
	assert.equal(fileExtension('.gitignore'), '');
	assert.equal(fileExtension('/a/.env'), '');
	assert.equal(fileExtension('a/.eslintrc.json'), '.json');
});
