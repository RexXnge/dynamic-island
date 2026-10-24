# Dynamic Island for Windows

An Apple-style Dynamic Island for Windows, written in Rust with Direct2D and DirectComposition. It renders only while something animates, so it idles at 0% CPU.

- Now playing with album art, waveform, seek bar and synced lyrics (LRCLIB, opt-in)
- Timer, clipboard notices and history, downloads, Focus, Bluetooth headphones
- Volume and brightness pill, mic-in-use dot
- File shelf (drag files in, drag them out later)
- Calculator and unit converter
- Themes with custom colours, size, corners, motion and per-pill settings
- Hides while a game or video is fullscreen; optional start with Windows

## Build

```
cargo build --release
```

Run `target/release/dynamic-island.exe`.

Installer: after the build above, `cargo build --release` in `installer/` makes `dynamic-island-setup.exe`, a per-user installer (no admin) with a Start menu shortcut and an uninstaller in Settings > Apps. Settings live in `%APPDATA%\DynamicIsland\settings.ini`.

## License

MIT, see [LICENSE](LICENSE).
