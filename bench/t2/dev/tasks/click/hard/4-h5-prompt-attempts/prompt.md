# Prompts: give up after a number of invalid answers

`click.prompt` and `click.confirm` ask again forever when the answer is invalid, which hangs scripts that pipe input in. Add a keyword parameter `max_attempts: int | None = None` to `click.prompt` (all its overloads) and to `click.confirm`, documented in both docstrings. With it, after that many invalid answers the prompt raises a new exception `click.TooManyAttempts` instead of asking again.

- An invalid answer is a value the conversion rejects, a confirmation that does not match (`confirmation_prompt`), or for `confirm` anything but yes/no. An empty answer re-asked because there is no default does not count.
- Each invalid answer still prints its error line. Before the last allowed one the line ends with how many attempts are left: `Error: 'many' is not a valid integer. (2 attempts left)`, `(1 attempt left)`; the error line of the last one has no such ending. Without `max_attempts` the output stays as it is. `max_attempts` below 1 raises `ValueError` at once, before anything is asked.
- `click.TooManyAttempts(attempts, last_error=None)` in `src/click/exceptions.py` (exported from `click`, an `autoexception` entry in `docs/api.md` after `BadArgumentUsage`, and listed in `docs/exceptions.md`) is a `ClickException` with exit code 1 and the message `Gave up after 3 invalid answers.` (`Gave up after 1 invalid answer.` for one). Its attributes are `attempts` and `last_error`, the error message of the last invalid answer (`'z' is not a valid integer.`). So inside a command it ends the program with `Error: Gave up after 3 invalid answers.` and exit code 1.
- `click.Option` takes `prompt_attempts: int | None = None` (documented) and passes it as `max_attempts` when it prompts, for normal options and for boolean flags that use `confirm`. `prompt_attempts` without `prompt` raises `TypeError`.

Add a section "Limiting Attempts" to `docs/prompts.md` with an example session.

Keep the existing behaviour otherwise. Finish the work completely.
