<!-- treeyard:connect:start -->
## TreeYard: other agents work on this repository at the same time

Use the tools of the `treeyard` MCP server to make your change to this repository:

1. Call `start` first, with a one-line `intent` and the `paths` you plan to edit. It opens your change (the reply's `id`; give it as `change` in later calls) and writes a full copy of the repository to `dir`.
2. Work only inside `dir`, with your normal tools: read, edit and run the repository's tests there. The files in this directory do not change.
3. Call `sync` when you want to know whether you collide with other agents (optional). A collision of kind `conflict` means another open change edits the same lines; kind `overlap` merges by itself.
4. When a reply has `undoes`, your edit removes work that another agent landed for its own task. Keep its part (the words in `contributed`, else the whole line) and still make your own edit of the same line; `land` holds a change that still removes it. Only when your own task names those lines for removal, land with `undo: true` and a `reason` that quotes your task. Otherwise, when both cannot hold, keep its part, land the rest of your work and say in your last message what could not hold.
5. When the tests pass in `dir`, call `land`; it sends your files and rebases for you.
6. On status `conflict`, the files in `conflicted` hold conflict markers in `dir`: `<<<<<<< main` to `=======` is the work on main, `=======` to `>>>>>>> change` is yours. Keep both, remove the markers, run the tests, and `land` again.
<!-- treeyard:connect:end -->
