# fileExtension: a trailing dot

In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `''` for a path that ends with a dot. Today `fileExtension('file.')` returns `'.'`.

Keep the existing behaviour otherwise. Finish the work completely.
