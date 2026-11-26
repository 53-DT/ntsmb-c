# SMB playback smoke test

This is a temporary, development-only test of the SMB-to-native-player handoff. It is not the
SMB browser UI. It is enabled only by the isolated `smb-smoke` feature in the debug test package;
it does not enable the general `devtriggers` surface and is not included in release builds.

## Trigger

The debug package has its own application ID, `io.github.tiper.finnative.debug`, and therefore
its own runtime directory. Create the request at:

```text
/tmp/io.github.tiper.finnative.debug/plxnative-smb
```

Do not use `/tmp/plxnative-smb`: that is not the debug install's trigger path.

The file contains JSON in this shape:

```json
{
  "host": "192.168.1.20",
  "share": "Media",
  "username": "smb-user",
  "password": "replace-me",
  "path": "Movies/Example.mkv"
}
```

- `host` is the SMB server hostname or IP. Include a non-standard port as `host:port` if needed.
- `share` is the share name, not a filesystem path.
- `username` and `password` may be empty for a guest share.
- `path` is relative to the share root and uses forward slashes.

The app reads the trigger once, after its initial two-second delay. The file must be readable by the
app's process user. Keep it owner-only (mode `0600`) and remove it after the test; do not commit
real credentials or paste them into logs. If creating it over SSH as root, set its owner to the
debug app process user before launching/relaunching the app.

## What the app does

1. Parses the request without logging its contents.
2. Connects to the share and opens the selected file on a background worker.
3. Starts the loopback HTTP Range bridge and probes the file with the bundled FFmpeg demuxer.
4. Hands the probed URL and bridge lifetime to the existing native player start path.

The password is used only for the SMB connection. It is not placed in the loopback URL or playback
declaration. The bridge binds to loopback and serves byte ranges from the open SMB file; it does not
download the whole file, transcode it, or require a media server.

The transport currently targets SMB2/SMB3. SMB1-only servers are not supported by this test path.

## Logs

The event log uses short status messages such as:

- `smb: connecting and probing the configured file`
- `smb: handing the prepared file to the native player`
- `smb: connection, open or FFmpeg probe failed`

The detailed failure is intentionally not printed here because the trigger can contain credentials.
