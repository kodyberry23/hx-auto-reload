# hx-auto-reload

A zellij plugin that watches the session's host folder for filesystem
changes and fires `:reload-all` in a [helix](https://helix-editor.com)
pane.

Replaces the per-language `fs_watcher_lsp` workaround with a single,
language-agnostic plugin that talks to helix over zellij's STDIN
injection API. No external watcher daemon, no LSP scaffolding.

## How it works

1. Subscribes to `FileSystemCreate / Update / Delete` events and calls
   `watch_filesystem()` once permissions are granted.
2. Tracks the helix pane's id from every `PaneUpdate` by matching on a
   configurable pane title (`"editor"` by default).
3. When a relevant filesystem event arrives, debounces briefly and
   injects `Esc → :reload-all → Enter` into the helix pane's STDIN.

The plugin renders nothing - drop it into a `size=1 borderless=true`
slot at the bottom of your layout (the status-bar pattern).

## Install

### Option 1 - zellij fetches the .wasm by URL (recommended)

Add this to your layout:

```kdl
pane size=1 borderless=true {
    plugin location="https://github.com/kodyberry23/hx-auto-reload/releases/latest/download/hx-auto-reload.wasm"
}
```

zellij downloads the .wasm on first launch and caches it under
`~/Library/Application Support/org.Zellij-Contributors.Zellij/plugins/`
(macOS) / `~/.cache/zellij/plugins/` (Linux).

### Option 2 - local file

```bash
mkdir -p ~/.config/zellij/plugins
curl -L https://github.com/kodyberry23/hx-auto-reload/releases/latest/download/hx-auto-reload.wasm \
  -o ~/.config/zellij/plugins/hx-auto-reload.wasm
```

```kdl
pane size=1 borderless=true {
    plugin location="file:~/.config/zellij/plugins/hx-auto-reload.wasm"
}
```

### Option 3 - build from source

```bash
rustup target add wasm32-wasip1
git clone https://github.com/kodyberry23/hx-auto-reload
cd hx-auto-reload
cargo build --release
# Built artifact: target/wasm32-wasip1/release/hx-auto-reload.wasm
```

## Configuration

Configuration is passed as KDL children of the plugin node:

```kdl
plugin location="..." {
    mode "all"               // "all" (default) or "scoped"
    debounce_ms "100"        // debounce window in ms; default 100
    editor_pane_title "editor"  // pane title to inject into; default "editor"
    ignore ".git/, node_modules/, target/"  // comma-separated; replaces defaults
    buffers_file "/tmp/helix-open-buffers.txt"  // only used in scoped mode
}
```

### Mode: `all` vs `scoped`

**`all`** - fire `:reload-all` on any non-noise filesystem change.
Simplest. Discards every unsaved edit in every buffer when it fires.
Right choice if you reliably save before running formatters, git pulls,
etc.

**`scoped`** - only fire if the changed path appears as a substring of
any line in `buffers_file`. You maintain that file from your editor
workflow (each `:open` appends; close-buffer removes - depends on your
setup). Tighter blast radius if you don't trust unsaved edits to be
safe.

If `mode = "scoped"` but `buffers_file` is missing or unset, the plugin
falls back to `all` semantics - your reload still works, it just isn't
narrowed.

### Pane targeting

The plugin needs to know which pane is your helix. By default it
matches on `title == "editor"`, so set your layout up like:

```kdl
pane name="editor" focus=true {
    command "hx"
}
```

zellij uses the KDL `name=` as the initial pane title. If helix later
sends OSC 0 to change the title (e.g. via a shell wrapper), the
plugin won't find it unless you also update `editor_pane_title` to
match, or have your wrapper restore the OSC 0 title to `"editor"`.

### Ignored paths

Defaults - substring matches against the full path:

```
/.git/    /node_modules/    /target/    /.cache/
/dist/    /build/           /.zellij/   .DS_Store
```

Plus path-suffix matches: `~`, `.swp`, `.tmp`, and filenames matching
`4913` (vim's atomic-rename probe) or `.#*` (emacs lockfiles).

Overriding `ignore` in config replaces this list entirely - if you want
to keep some defaults plus add your own, include them in your `ignore`
string.

## Caveats

- **`:reload-all` is destructive.** Any unsaved edit in any buffer is
  discarded. Use `scoped` mode + a curated `buffers_file` to narrow
  the blast radius.
- **No "is helix dirty?" check.** helix doesn't expose its buffer dirty
  state to zellij plugins, so we can't gate reload on "only when clean."
- **The plugin pane is invisible but real.** It occupies a 1-row slot
  and shows up in `zellij action list-panes`. If you use
  `swap_tiled_layout` blocks, count it in your `exact_panes=N`
  constraints.

## License

MIT. See [LICENSE](./LICENSE).
