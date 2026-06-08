# 🦅 FOLLOW

`follow` is a TUI tool for tracking and understanding the commands your coding agents run. It intercepts every command and queries a local [`explainshell`](https://github.com/idank/explainshell) server to display explanations in a sidepane you can open with `C-f`.

<img width="800" height="450" alt="flw_optimized" src="https://github.com/user-attachments/assets/c0952e78-9a60-4546-bfdb-05f3199a6520" />

## Quick Start

```bash
# Install
cargo install --git https://github.com/emilschmitz/follow.git --bin flw

# Run
flw claude  # (or whatever agent you prefer)
```
Toggle explanations inside the shell by pressing `Ctrl+F`.

## Installation

If you have Rust and Cargo installed, you can compile and install `follow` directly from the Git repository:

```bash
cargo install --git https://github.com/emilschmitz/follow.git --bin flw
```

This will build the binary and place it in your local cargo bin directory (`~/.cargo/bin/flw`), which is typically already in your shell's `PATH`.

## Feedback

If you've tried this tool out, I'd appreciate your feedback! Send an email to `[first-initial].schmitz at outlook.com`.
