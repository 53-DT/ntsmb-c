# DeckFiles — new UI and implementation boundary

## Product identity
DeckFiles is a standalone, lightweight file browser for local storage and SMB shares. It is not a Plex/Jellyfin client, media catalog, or home-theater shell.

## Design direction (fresh UI)
Design the DeckFiles screens from a blank page. Use a clean, contemporary TV-friendly layout inspired only by the visual language of FinNative's sign-in screen (typography, field treatment, spacing, remote-focus clarity). Do not copy old NT-SMB screen layouts or transplant a screen's code.

- New DeckFiles branding and screen composition.
- Dark, calm background; clear contrast; restrained accent; no posters, thumbnails, hero banners, artwork, metadata, cards-as-media-library, or decorative content.
- Large couch-readable labels and input text; keep all primary controls within safe screen bounds.
- Remote focus is obvious, consistent and stable. Focus moves predictably between form fields, primary actions, saved connection rows, browser list and back controls.
- Use a vertical file/folder list, not a poster/tile/grid layout. Folder rows are visually distinct from video rows with simple icons; file names remain legible and clipped safely.
- Browser header presents connection/share and breadcrumb/path context. Loading, empty, permission, connection and error states are explicit and nontechnical where possible.
- Keep transitions and animation subtle and functional; no flashy motion.

## Screen flow
1. **Connect** — host/IP, port (445 default), username, password, Guest toggle, Connect. Host and port are distinct inputs. Saved connections may be shown as simple named rows, not media cards.
2. **Share selection** — simple list of shares, selected with the remote, with clear connection context and Back.
3. **Browse** — plain folder-first list; enter folder, parent/back, restore cursor when returning, play video by selecting it.
4. **Playback** — use the existing native FinNative player and SMB range bridge. No new player UI.
5. **Settings** — only the small app-level options required for DeckFiles (connection management and playback-related defaults); keep separate from inherited Plex/Jellyfin settings.

## Hard code boundary
- Write DeckFiles UI/screen logic fresh; do not copy or adapt the old SMB screen's layout, drawing, grid/card/avatar-row, profile controls, form arrangement or state implementation.
- Reuse only proven shared infrastructure where it is a neutral utility/component: remote input/focus engine, text input/keyboard integration, standard button/text primitives, safe layout/measurement, animation/transition, and the existing native player handoff.
- Keep SMB transport, validation, worker, controller, safe path/browser model, range bridge and secure credential storage as domain/core code. They are not UI code and can be reused after review.
- Remove Plex/Jellyfin-only routes, catalog/account/session settings and UI dependencies from the standalone DeckFiles target rather than hiding them. Do not remove shared codecs, platform services or player infrastructure needed by DeckFiles.
- No credentials in logs, recorder/replay state, URLs, debug strings or plaintext persistence. Guest mode has no password.

## Build/test gates
- Each implementation milestone: inspect the diff for accidental inherited UI or unrelated files; run relevant Rust tests/checks; build the ARM webOS IPK with GitHub Actions; review actual failure logs if it fails.
- TV acceptance is separate from CI: connect to a real SMB server, browse nested folders and back with focus restoration, play a video, seek, stop/return, and reconnect.
