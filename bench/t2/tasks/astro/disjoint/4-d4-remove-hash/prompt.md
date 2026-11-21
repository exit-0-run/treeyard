# removeQueryString: remove the hash too

In `packages/internal-helpers/src/path.ts`, make `removeQueryString` also remove a hash. Examples: `removeQueryString('/a/b.png?w=10#x')` and `removeQueryString('/a/b.png#x')` are `'/a/b.png'`.

Keep the existing behaviour otherwise. Finish the work completely.
