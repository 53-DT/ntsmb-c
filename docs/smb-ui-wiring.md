# NT-SMB UI wiring

## Boundary

The screens crate remains sans-IO. A Connect/Browse screen owns only form presentation,
focus declarations, and the last value-model revision it drew. It must not construct an
`SmbWorker`, call `BrowseController::pump`, read a filesystem path, or name
`crate::app` or a sibling screen.

The SMB transport crate (`plx_smb`) owns the pure browser model and the
`BrowseController` implementation. The application owns the live controller instance,
its lifetime, saved-profile store, and all execution of SMB work. The controller may own
the worker thread; that ownership is deliberately above the screen layer.

## Screen -> application requests

Add typed SMB requests to `screens::registry::AppFx`, with matching
`ExternalAppFx` variants when a request needs the application/resource runner rather
than the inline logical executor. The Connect screen emits a connect request carrying a
validated `ConnectRequest`; the Browse screen emits share selection, directory activation,
Back, disconnect, and playback-preparation requests. The screen only describes intent.
The bridge/application runner performs the controller call and any platform/player work.

Credential-bearing requests are live-only: do not add them to recorder/replay codecs,
debug formatting, logs, URLs, or digest state. Password text stays in the form until
validation, then only in `ConnectRequest` / the worker-owned request. Persist only a
sealed `CredentialEnvelope`; Guest profiles contain no password.

## Application -> screen publication

Publish a small credential-free SMB view model through addressed `AppMsg` events to
the owning screen machine (or its owned subscription channel if the existing page
pattern requires one). The published state contains phase, share names, current
location, visible browser rows, selected row, loading/error text, and a monotonically
increasing revision. It must not contain the password, raw request, credential envelope,
or any debug representation of them.

The bridge is the only place that translates controller state into this view model.
The screen compares the revision during its normal machine step and invalidates only
when the published presentation changes. Follow the existing Search/Library publication
and addressed-event patterns rather than letting a screen borrow the controller or
application stores.

## Per-frame pump

The application/Bridge frame path calls `BrowseController::pump()` exactly once per
normal frame while the SMB app owns an active connection. Place it in the Bridge's
existing per-frame owner-pump section, alongside the other non-blocking pumps
(e.g. Search/Watchlist and the browse/store drains), not in a screen's `step` or
`draw`. `pump()` only drains already-completed worker events; it must never block.

When it reports a change, the Bridge snapshots the controller's public presentation
and sends an addressed SMB state message to the active SMB screen. Screen damage is
revision-driven. Controller construction, connect, directory listing, and playback
preparation are initiated by application effects, never by a screen machine.

## Playback handoff

The browser's activation may produce a `PlayRequest`. The app starts
`plx_smb::controller::spawn_prepare` off-frame, then hands a completed
`PreparedPlayback` to the existing playback path. Share the handoff implementation
with the retained `smb_playback_arm` smoke scenario. Do not change the player,
demuxer, range bridge, or media path. Keep the smoke trigger as a transport regression
test.

## Build/test sequence

1. Land and compile the SMB controller/store modules and module-layer declarations.
2. Land this wiring note before any screen implementation.
3. Prove the SMB app boot/root route with a minimal screen.
4. Add Connect using the Login form's TextBuffer/TextField/secret-entry pattern.
5. Add share/folder browsing and the existing playback handoff.
6. Add sealed saved-server persistence and cold-reboot verification.

Each stage ends with the ARM debug build and its IPK artifact. TV-only claims (storage
writability, cold reboot persistence, real authentication, and playback) remain unproven
until tested on the television.
