//! hx-auto-reload - a zellij plugin that fires `:reload-all` in a helix
//! pane when files change on disk inside the session's host folder.
//!
//! Why this exists: helix's `fs_watcher_lsp` workaround for on-disk
//! changes is per-language and unreliable in practice. zellij plugins
//! have first-class `FileSystem*` events and direct STDIN injection
//! into named panes, so the same goal becomes a small wasm plugin
//! with no LSP scaffolding.
//!
//! How it works:
//!  1. On load, request `ReadApplicationState` + `WriteToStdin`
//!     permissions, subscribe to filesystem + pane events, and call
//!     `watch_filesystem()` once the user grants permission.
//!  2. From every `PaneUpdate` event, snapshot the id of the terminal
//!     pane whose title matches `editor_pane_title` (default "editor"),
//!     so we always know where to inject keystrokes.
//!  3. When `FileSystemCreate`/`Update`/`Delete` fires, ignore noise
//!     paths (`.git/`, `node_modules/`, vim swap files, etc.) and, in
//!     scoped mode, paths not in the user's open-buffer list. If any
//!     surviving path warrants a reload, arm a debounce timer.
//!  4. When the timer expires, inject `Esc → :reload-all → Enter` into
//!     the editor pane via `write_to_pane_id`.
//!
//! Render is a no-op - the plugin is meant to live in a `size=1
//! borderless=true` pane (status-bar pattern) and never draw anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zellij_tile::prelude::*;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    /// Any non-noise filesystem change triggers `:reload-all`.
    All,
    /// Only changes to files in `scoped_buffers_path` trigger.
    /// Falls back to `All` semantics if the path isn't configured or
    /// the file doesn't exist yet.
    Scoped,
}

struct AutoReload {
    permission_granted: bool,
    /// Terminal pane id of the helix pane, learned from PaneUpdate events.
    editor_pane_id: Option<u32>,
    /// Pane title to match against (set via `editor_pane_title` config,
    /// defaults to "editor").
    editor_pane_title: String,
    /// True if a Timer event is pending and will fire the reload.
    pending: bool,
    mode: Mode,
    debounce_ms: u64,
    /// Substring matches that mark a path as "noise" and skip the reload.
    /// Comma-separated `ignore` config replaces this list entirely.
    ignore_substrings: Vec<String>,
    /// In `scoped` mode, plugin reads this file to decide whether a
    /// changed path is "open in helix". Each line is a path substring
    /// to match against.
    scoped_buffers_path: Option<PathBuf>,
}

impl Default for AutoReload {
    fn default() -> Self {
        Self {
            permission_granted: false,
            editor_pane_id: None,
            editor_pane_title: "editor".to_string(),
            pending: false,
            mode: Mode::All,
            debounce_ms: 100,
            ignore_substrings: default_ignores(),
            scoped_buffers_path: None,
        }
    }
}

fn default_ignores() -> Vec<String> {
    [
        "/.git/",
        "/node_modules/",
        "/target/",
        "/.cache/",
        "/dist/",
        "/build/",
        "/.zellij/",
        ".DS_Store",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

register_plugin!(AutoReload);

impl ZellijPlugin for AutoReload {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        if let Some(v) = configuration.get("debounce_ms") {
            if let Ok(n) = v.parse::<u64>() {
                self.debounce_ms = n;
            }
        }
        if let Some(v) = configuration.get("mode") {
            self.mode = match v.as_str() {
                "scoped" => Mode::Scoped,
                _ => Mode::All,
            };
        }
        if let Some(v) = configuration.get("editor_pane_title") {
            if !v.is_empty() {
                self.editor_pane_title = v.clone();
            }
        }
        if let Some(v) = configuration.get("buffers_file") {
            if !v.is_empty() {
                self.scoped_buffers_path = Some(PathBuf::from(v));
            }
        }
        if let Some(v) = configuration.get("ignore") {
            // Comma-separated. Empty string → keep defaults; one or more
            // non-empty entries → replace defaults entirely. Compose your
            // own list including any defaults you still want.
            let parsed: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !parsed.is_empty() {
                self.ignore_substrings = parsed;
            }
        }

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::WriteToStdin,
        ]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::PaneUpdate,
            EventType::FileSystemCreate,
            EventType::FileSystemUpdate,
            EventType::FileSystemDelete,
            EventType::Timer,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                if matches!(status, PermissionStatus::Granted) {
                    self.permission_granted = true;
                    // watch_filesystem starts delivering FS events for
                    // the session's host folder (recursively).
                    watch_filesystem();
                }
            }
            Event::PaneUpdate(manifest) => {
                self.editor_pane_id = find_editor_pane(&manifest, &self.editor_pane_title);
            }
            Event::FileSystemCreate(paths)
            | Event::FileSystemUpdate(paths)
            | Event::FileSystemDelete(paths) => {
                if self.permission_granted
                    && self.editor_pane_id.is_some()
                    && !self.pending
                    && self.any_relevant(&paths)
                {
                    self.pending = true;
                    set_timeout(self.debounce_ms as f64 / 1000.0);
                }
            }
            Event::Timer(_) => {
                if !self.pending {
                    return false;
                }
                self.pending = false;
                if let Some(id) = self.editor_pane_id {
                    let target = PaneId::Terminal(id);
                    // Esc normalizes helix to normal mode (no-op if
                    // already there); :reload-all then Enter executes
                    // the command. If the user is in insert mode with
                    // unsaved edits, those edits are discarded by
                    // :reload-all - that's the intentional tradeoff of
                    // `mode = "all"`. Use `mode = "scoped"` plus a
                    // buffers_file to narrow the trigger.
                    write_to_pane_id(vec![0x1b], target.clone());
                    write_chars_to_pane_id(":reload-all", target.clone());
                    write_to_pane_id(vec![0x0d], target);
                }
            }
            _ => {}
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        // Intentionally blank. The plugin's pane is decorative - give
        // it `size=1 borderless=true` in your layout.
    }
}

impl AutoReload {
    fn any_relevant(&self, paths: &[(PathBuf, Option<FileMetadata>)]) -> bool {
        for (path, _) in paths {
            if self.is_ignored(path) {
                continue;
            }
            match self.mode {
                Mode::All => return true,
                Mode::Scoped => {
                    if self.is_in_buffers(path) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let s = path.to_string_lossy();
        // Editor swap / temp file conventions.
        if s.ends_with('~') || s.ends_with(".swp") || s.ends_with(".tmp") {
            return true;
        }
        // 4913 is vim's atomic-rename probe.
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == "4913" || name.starts_with(".#") {
                return true;
            }
        }
        self.ignore_substrings
            .iter()
            .any(|sub| s.contains(sub.as_str()))
    }

    fn is_in_buffers(&self, path: &Path) -> bool {
        let Some(buffers_path) = &self.scoped_buffers_path else {
            // Scoped mode requested but no buffer list configured - fail
            // open (behave like All) so the user isn't silently broken.
            return true;
        };
        let Ok(contents) = std::fs::read_to_string(buffers_path) else {
            return true;
        };
        let target = path.to_string_lossy();
        contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .any(|line| target.contains(line))
    }
}

fn find_editor_pane(manifest: &PaneManifest, title: &str) -> Option<u32> {
    for panes in manifest.panes.values() {
        for pane in panes {
            if !pane.is_plugin && pane.title == title {
                return Some(pane.id);
            }
        }
    }
    None
}
