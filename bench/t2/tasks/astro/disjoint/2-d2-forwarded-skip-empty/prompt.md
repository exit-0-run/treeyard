# getFirstForwardedValue: skip empty entries

In `packages/internal-helpers/src/request.ts`, make `getFirstForwardedValue` skip empty entries of the header. Example: `getFirstForwardedValue(' , ,1.2.3.4, 5.6.7.8')` is `'1.2.3.4'`. When every entry is empty, it still returns `''`.

Keep the existing behaviour otherwise. Finish the work completely.
