# matchProtocol: accept a trailing colon

In `packages/internal-helpers/src/remote.ts`, make `matchProtocol` accept a protocol written with its trailing colon. Example: `matchProtocol(new URL('https://a.com'), 'https:')` is true, the same as with `'https'`.

Keep the existing behaviour otherwise. Finish the work completely.
