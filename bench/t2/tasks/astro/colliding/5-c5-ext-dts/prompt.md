# fileExtension: .d.ts

In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `'.d.ts'` for a TypeScript declaration file. Example: `fileExtension('/types/env.d.ts')` is `'.d.ts'`; `fileExtension('/a/b.ts')` stays `'.ts'`.

Keep the existing behaviour otherwise. Finish the work completely.
