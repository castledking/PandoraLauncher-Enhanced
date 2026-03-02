#!/bin/bash
exec env -i \
  HOME="$HOME" \
  USER="$USER" \
  PATH="$HOME/.cargo/bin:/usr/bin:/usr/local/bin:/bin" \
  RUSTUP_HOME="$HOME/.rustup" \
  CARGO_HOME="$HOME/.cargo" \
  DISPLAY="${DISPLAY:-:0}" \
  WAYLAND_DISPLAY="${WAYLAND_DISPLAY}" \
  XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR}" \
  "$HOME/.cargo/bin/cargo" "$@"
