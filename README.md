# 🦅 flw

`flw` is a terminal user interface (TUI) tool that runs side-by-side with your agent or shell terminal. It tracks the commands you run and queries a local `explainshell` server to display explanations of all the components of the commmands the agent's has run.

## Quick Start

```bash
# Install
cargo install --git https://github.com/emilschmitz/follow.git --bin flw

# Run
flw claude  # (or whatever agent you prefer)
```
Toggle explanations inside the shell by pressing `Ctrl+F`.

https://github.com/emilschmitz/follow/assets/.../demo.mp4

---

## Installation

If you have Rust and Cargo installed, you can compile and install `flw` directly from the Git repository:

```bash
cargo install --git https://github.com/emilschmitz/follow.git --bin flw
```

This will build the binary and place it in your local cargo bin directory (`~/.cargo/bin/flw`), which is typically already in your shell's `PATH`.

