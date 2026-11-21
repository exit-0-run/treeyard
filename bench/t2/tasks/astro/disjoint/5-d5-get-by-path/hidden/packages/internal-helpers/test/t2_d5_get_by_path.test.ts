import assert from 'node:assert/strict';
import { it } from 'node:test';
import { getByPath } from '../src/object.ts';

it('reads a dot path and refuses forbidden keys', () => {
	assert.equal(getByPath({ a: { b: { c: 3 } } }, 'a.b.c'), 3);
	assert.equal(getByPath({ a: 1 }, 'a.x'), undefined);
	assert.equal(getByPath({}, '__proto__.polluted'), undefined);
	assert.equal(getByPath({ a: { constructor: 1 } }, 'a.constructor'), undefined);
});
