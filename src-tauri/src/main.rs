#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|arg| arg == "--once") {
        claude_usage_menubar::debug_once();
        return;
    }
    claude_usage_menubar::run();
}
