# fileExtension: ignore the query and the hash

In `packages/internal-helpers/src/path.ts`, make `fileExtension` ignore a query string and a hash. Examples: `fileExtension('/a/b.js?v=1')` is `'.js'`, `fileExtension('/x/y.css#top')` is `'.css'`.

Keep the existing behaviour otherwise. Finish the work completely.
