# CliRunner: check the exit code

Tests that use `click.testing.CliRunner` repeat `assert result.exit_code == 0, result.output`. Add a method `Result.check(exit_code: int = 0) -> Result` in `src/click/testing.py`: it returns the result itself when its `exit_code` equals `exit_code`, and otherwise raises `AssertionError` with a message that starts with `exit code 2, expected 0` (the actual and the expected code), then holds, on lines of their own, `exception: <repr of the exception>` when the result holds an exception other than `SystemExit`, and `output:` followed by the last 20 lines of the output. Also add a keyword parameter `check: bool = False` to `CliRunner.invoke` that calls `check()` on the result before returning it. Document both.

Keep the existing behaviour otherwise. Finish the work completely.
