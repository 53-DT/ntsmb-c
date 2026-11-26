# NT-SMB — Native SMB File Browser for LG webOS

A simple, native SMB file browser and video player for LG webOS TVs.

The goal is to connect directly to SMB shares, browse folders and files with the TV remote, and play video files through the TV's native playback pipeline. No Plex or Jellyfin server is required.

## Intended UI

- Plain, practical file-browser layout — folders and filenames, not media-library shelves.
- Straightforward SMB connection/profile settings.
- Fin Native-derived polished controls: rounded Apple-TV-like buttons, setting panels, focus movement/highlight, animations and remote navigation.
- Keep the settings and control styling/interaction polish; **do not copy the original Plex/Jellyfin home, library, poster, catalog or detail-page UI**.
- Select a video file to play; retain seek support where the SMB range stream allows it.

## Status

**Work in progress.** The SMB transport and range-streaming foundation are in place, and browser helper logic (folder-first sorting, video filtering and safe relative navigation) has been added with unit tests. The actual TV browser screen, connection/settings UI, credential storage and end-to-end playback still need implementation and testing.

Development branch: [`smb-development`](https://github.com/davetrump53-ai/ntsmb/tree/smb-development)

## Attribution and licensing

This project reuses code from PlxNative/Fin Native. See [LICENSE](LICENSE), [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md), and source-level notices for attribution and licensing details. This project is independent and is not affiliated with Plex, Jellyfin, LG or the upstream projects.

## Development direction

See [SMB UI plan](docs/smb-ui-parity-plan.md) for the agreed scope and implementation milestones.
