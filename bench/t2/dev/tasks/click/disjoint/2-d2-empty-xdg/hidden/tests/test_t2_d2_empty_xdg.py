import os
import sys

import click


def test_an_empty_xdg_config_home_counts_as_unset(monkeypatch, tmp_path):
    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(click.utils, "WIN", False)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("XDG_CONFIG_HOME", "")
    assert click.get_app_dir("Foo Bar") == os.path.join(str(tmp_path), ".config", "foo-bar")
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "cfg"))
    assert click.get_app_dir("Foo Bar") == os.path.join(str(tmp_path / "cfg"), "foo-bar")
