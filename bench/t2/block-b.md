<!-- treeyard:connect:start -->
## TreeYard: other agents work on this repository at the same time

Use the tools of the `treeyard` MCP server for every change to this repository:

1. Call `open_change` with a one-line `intent` and the `paths` you plan to edit.
2. Edit files only through `apply`, with `replace` edits where you can. Do not write the files on disk. `read` shows main; give `at` = your change id to see your own edits.
3. When a reply has `undoes`, your edit removes lines that another agent landed for its own task. Put them back and apply again, unless your task names those lines for removal.
4. When `apply` returns a collision of kind `conflict`, another open change edits the same lines: adjust your edit and apply again. A collision of kind `overlap` merges by itself and needs no action.
5. Test before you land: call `checkout` with your change id. It writes your change into a directory of its own and returns its path in `dir`; run the repository's tests there. Later `apply` calls update that directory. Edits made in it do not count: fix code with `apply`.
6. Finish with `land`. On status `conflict`, replace each region in `change.conflicts` with its final text, all regions of a file in one `apply`, then `land` again.
7. The files in this directory do not change when you land. To see landed work, use `read`.
<!-- treeyard:connect:end -->
