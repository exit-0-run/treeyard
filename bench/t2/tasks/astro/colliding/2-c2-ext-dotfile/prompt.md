# fileExtension: a dotfile has no extension

In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `''` for a file whose name starts with a dot and has no other dot. Examples: `fileExtension('.gitignore')` and `fileExtension('/a/.env')` are `''`; `fileExtension('a/.eslintrc.json')` stays `'.json'`.

Keep the existing behaviour otherwise. Finish the work completely.
