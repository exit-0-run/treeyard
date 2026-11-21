<!-- treeyard:connect:start -->
## TreeYard: other agents work on this repository at the same time

Use the tools of the `treeyard` MCP server for every change to this repository:

1. Call `open_change` with a one-line `intent` and the `paths` you plan to edit. Read `overlaps` and `advice` in the reply.
2. Read little, in few calls. Find the place with one `grep` or `symbols` call, then read it with one `read` call with `start` and `end` that covers the whole function and some lines around it. Every call costs: do not page through a file in small pieces, and do not read a whole file of more than 300 lines.
3. Edit files only through `apply`, with `replace` edits where you can. Do not write the files on disk.
4. Lines on main that you did not write are the work of another agent's task. Keep them, even when they seem to go against your task: make your change hold next to them. When `apply` returns a non-empty `undoes`, your edit removes such lines: put them back and apply again, unless your task names those lines for removal.
5. When `apply` returns `collisions` of kind `overlap` or `conflict`, fix it at once: read the other change with `change`, adjust your edit, apply again.
6. Test before you land. Copy the files that the tests need into a new temporary directory: your edited files come from `read` with `at` set to your change id. Run the tests there.
7. Finish with `land`. On status `conflict`, write the final content of each task in `change.conflicts` with `apply` (or `resolve`), then `land` again.
8. The files in this directory do not change when you land. To see landed work, use `read`.
<!-- treeyard:connect:end -->
