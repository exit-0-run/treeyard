# object: getByPath

In `packages/internal-helpers/src/object.ts`, add an exported function `getByPath(obj, path)` that reads a dot-separated property path such as `'a.b.c'`. It returns `undefined` when a step is missing or when a step is one of `FORBIDDEN_PATH_KEYS`. Example: `getByPath({ a: { b: { c: 3 } } }, 'a.b.c')` is 3; `getByPath({}, '__proto__.x')` is `undefined`.

Keep the existing behaviour otherwise. Finish the work completely.
