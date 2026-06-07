# flw — TODO

- [ ] **Sizing** — right pane currently does a plain 50/50 split. Make the width configurable (e.g. `flw agi --width 40`) and persist the last-used width.

- [ ] **TUI polish** — inspired by [Atuin](https://github.com/atuinsh/atuin): fuzzy-searchable command list, timestamps, exit codes, syntax highlighting of commands, smooth scrolling feel.

- [ ] **ExplainShell integration** — when a command is selected in the right pane, hit Enter or Space to choose/expand the command, showing more detailed information (formatted cleanly from the JSON structure), while remaining escapable with Ctrl+F.

- [ ] **Fallback Help Content** — when ExplainShell returns a "missing man page" error (e.g. for new commands like `uv`), fall back to dynamically using an LLM or, alternatively, querying the command's own `--help` output and formatting/displaying it in the explanation pane.

- [ ] **ExplainShell Option Splitting** — when explainshell groups multiple options into a single help-box block (e.g. returning options `-l`, `-m`, `-n` all inside a single block for Plan 9 `ls`), write a custom post-processor/parser to isolate and extract only the relevant selected option flag description rather than displaying the whole block list.

- [ ] **Freestanding Nested Tmux Session** — when running inside an existing host Tmux session, explore using an isolated socket approach (`tmux -S`) to run a nested, freestanding session instead of injecting windows into the host session, and apply the stripped-down status-bar-free UI settings there too.
