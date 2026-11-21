from click._termui_impl import Editor


def test_blank_editor_variables_are_skipped(monkeypatch):
    monkeypatch.setenv("VISUAL", "  ")
    monkeypatch.setenv("EDITOR", "nano")
    assert Editor().get_editor() == "nano"
    monkeypatch.setenv("VISUAL", "vim")
    assert Editor().get_editor() == "vim"
