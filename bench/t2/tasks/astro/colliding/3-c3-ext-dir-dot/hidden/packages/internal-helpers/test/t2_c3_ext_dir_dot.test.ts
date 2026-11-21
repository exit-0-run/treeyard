import assert from 'node:assert/strict';
import { it } from 'node:test';
import { fileExtension } from '../src/path.ts';

it('ignores a dot in a directory name', () => {
	assert.equal(fileExtension('/a.b/c'), '');
	assert.equal(fileExtension('C:\\a.b\\c'), '');
	assert.equal(fileExtension('/a.b/c.md'), '.md');
});
