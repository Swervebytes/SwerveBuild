// Single source of truth for the app version shown in the UI.
//
// Injected at build time from package.json via vite.config.js `define`, so a
// manifest bump (package.json / Cargo.toml / tauri.conf.json) can't drift from
// the version rendered in the sidebar and Settings → About again. The Rust side
// derives its version the same way, from `env!("CARGO_PKG_VERSION")`.
declare const __APP_VERSION__: string;

export const APP_VERSION: string = __APP_VERSION__;
