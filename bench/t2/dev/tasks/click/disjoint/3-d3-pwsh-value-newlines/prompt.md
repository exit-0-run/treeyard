# PowerShell completion: newlines in the value

In `src/click/shell_completion.py`, `PowerShellComplete.format_completion` collapses newlines in the help text but not in the value, so a value with a newline breaks the framing of three lines per item. Replace `\r` and `\n` in the value with spaces too. Example: for `CompletionItem("a\nb")` it returns `"plain\na b\n_"`.

Keep the existing behaviour otherwise. Finish the work completely.
