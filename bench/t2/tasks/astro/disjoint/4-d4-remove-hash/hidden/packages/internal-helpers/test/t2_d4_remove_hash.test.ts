import assert from 'node:assert/strict';
import { it } from 'node:test';
import { removeQueryString } from '../src/path.ts';

it('removes the query and the hash', () => {
	assert.equal(removeQueryString('/a/b.png?w=10#x'), '/a/b.png');
	assert.equal(removeQueryString('/a/b.png#x'), '/a/b.png');
	assert.equal(removeQueryString('/a?b'), '/a');
});
