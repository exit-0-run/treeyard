# fileExtension: a dot in a directory name

In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `''` when the last dot is in a directory name, not in the file name (with `/` or `\` as the separator). Examples: `fileExtension('/a.b/c')` is `''`, `fileExtension('C:\\a.b\\c')` is `''`.

Keep the existing behaviour otherwise. Finish the work completely.
