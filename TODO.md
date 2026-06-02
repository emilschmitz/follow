# flw — TODO

- [ ] **Sizing** — right pane currently does a plain 50/50 split. Make the width configurable (e.g. `flw agi --width 40`) and persist the last-used width.

- [ ] **TUI polish** — inspired by [Atuin](https://github.com/atuinsh/atuin): fuzzy-searchable command list, timestamps, exit codes, syntax highlighting of commands, smooth scrolling feel.

- [ ] **ExplainShell integration** — when a command is selected in the right pane, hit Enter or Space to choose/expand the command, showing more detailed information (formatted cleanly from the JSON structure), while remaining escapable with Ctrl+F.

- [ ] **Fallback Help Content** — when ExplainShell returns a "missing man page" error (e.g. for new commands like `uv`), fall back to dynamically using an LLM or, alternatively, querying the command's own `--help` output and formatting/displaying it in the explanation pane.

