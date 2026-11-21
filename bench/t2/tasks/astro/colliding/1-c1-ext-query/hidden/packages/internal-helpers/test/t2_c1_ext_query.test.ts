import assert from 'node:assert/strict';
import { it } from 'node:test';
import { fileExtension } from '../src/path.ts';

it('ignores the query and the hash', () => {
	assert.equal(fileExtension('/a/b.js?v=1'), '.js');
	assert.equal(fileExtension('/x/y.css#top'), '.css');
});
