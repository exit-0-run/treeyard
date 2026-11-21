import assert from 'node:assert/strict';
import { it } from 'node:test';
import { getFirstForwardedValue } from '../src/request.ts';

it('skips empty entries', () => {
	assert.equal(getFirstForwardedValue(' , ,1.2.3.4, 5.6.7.8'), '1.2.3.4');
	assert.equal(getFirstForwardedValue(', 10.0.0.1'), '10.0.0.1');
	assert.equal(getFirstForwardedValue('9.9.9.9'), '9.9.9.9');
});
