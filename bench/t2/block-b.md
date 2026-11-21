<!-- treeyard:connect:start -->
## TreeYard: other agents work on this repository at the same time

Use the tools of the `treeyard` MCP server for every change to this repository:

1. Make your first edit with `apply`: give a one-line `intent`, the `paths` you plan to edit and your `edits`, and no `change`. It opens your change (the reply's `id`; give it as `change` in later calls) and writes your change into a directory of its own (`checkout.dir`).
2. Edit files only through `apply`, with `replace` edits where you can. Do not write the files on disk. `read` shows main; give `at` = your change id to see your own edits.
3. When a reply has `undoes`, your edit removes work that another agent landed for its own task. Keep its part (the words in `contributed`, else the whole line) and still make your own edit of the same line, then apply again. When your task and its part cannot both hold, keep its part, land the rest of your work and say so in your last message: a person decides. `land` holds a change that still removes it.
4. When `apply` returns a collision of kind `conflict`, another open change edits the same lines: adjust your edit and apply again. A collision of kind `overlap` merges by itself and needs no action.
5. Test before you land: run the repository's tests in `checkout.dir`. Later `apply` calls update that directory. Edits made in it do not count: fix code with `apply`. Rebase only when a checkout reply says `main_moved`.
6. Finish with `land`; it rebases for you. On status `conflict`, replace each region in `change.conflicts` with its final text, all regions of a file in one `apply`, then `land` again.
7. The files in this directory do not change when you land. To see landed work, use `read`.
<!-- treeyard:connect:end -->
