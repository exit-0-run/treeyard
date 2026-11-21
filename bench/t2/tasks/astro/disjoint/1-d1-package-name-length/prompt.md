# validatePackageName: at most 214 characters

In `packages/internal-helpers/src/cli.ts`, make `validatePackageName` return false for a name longer than 214 characters (the npm limit). A name of 214 characters stays valid.

Keep the existing behaviour otherwise. Finish the work completely.
