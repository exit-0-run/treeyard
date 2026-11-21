import assert from 'node:assert/strict';
import { it } from 'node:test';
import { matchProtocol } from '../src/remote.ts';

it('accepts a trailing colon', () => {
	assert.equal(matchProtocol(new URL('https://a.com'), 'https:'), true);
	assert.equal(matchProtocol(new URL('https://a.com'), 'https'), true);
	assert.equal(matchProtocol(new URL('https://a.com'), 'http:'), false);
});
