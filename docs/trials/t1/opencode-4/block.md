<!-- treeyard:connect:start -->
## TreeYard: other agents work on this repository at the same time

Use the tools of the `treeyard` MCP server for every change to this repository:

1. Call `open_change` with a one-line `intent` and the `paths` you plan to edit. Read `overlaps` and `advice` in the reply.
2. Edit files only through `apply` (or `replace` edits in `apply`). Do not write the files on disk.
3. When `apply` returns `collisions` of kind `overlap` or `conflict`, fix it at once: read the other change with `change`, adjust your edit, apply again.
4. Test before you land. Copy the files that the tests need into a new temporary directory: your edited files come from `read` with `at` set to your change id. Run the tests there.
5. Finish with `land`. On status `conflict`, write the final content of each task in `change.conflicts` with `apply` (or `resolve`), then `land` again.
6. The files in this directory do not change when you land. To see landed work, use `read`.
<!-- treeyard:connect:end -->
