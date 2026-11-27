//! Standalone DeckFiles TV runtime.
//!
//! This process loop owns the DeckFiles UI, asynchronous SMB operations and the native SMB player
//! handoff. It does not enter the legacy Plex/Jellyfin application composition.

use deckfiles_app::{DeckFilesAction, DeckFilesApp, DeckFilesInput, DeckFilesPage};
use deckfiles_screens::{deck_player::PlayerSnapshot, files_browser::FileItem};
use plx_media::player::{self, adapter::PlayerAdapter, machine::Player};
use plx_smb::provider::PreparedSourcePlay;
use plx_ui::{Painter, Rect};
use plxnative_modules::host::{self, Exit, MainThread, OpenAsk, Release, Shell, Surface};
use plx_ui::consts::{classify, Key};
use plx_shell_sdl::event::{Decoded, Lifecycle};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

extern "C" { fn glViewport(x: i32, y: i32, width: i32, height: i32); }

enum WorkerEvent {
    Shares(Result<Vec<String>, String>),
    Connected(Result<String, String>),
    Directory { source_id: String, path: String, result: Result<Vec<FileItem>, String> },
    Prepared(Result<PreparedSourcePlay, String>),
}

fn spawn_action(action: DeckFilesAction, data_dir: PathBuf, tx: &Sender<WorkerEvent>) -> Result<(), String> {
    match action {
        DeckFilesAction::None | DeckFilesAction::SaveSettings => Ok(()),
        DeckFilesAction::DiscoverShares(payload) => {
            let tx=tx.clone();
            std::thread::Builder::new().name("deckfiles-smb-shares".into()).spawn(move || {
                let result=plx_smb::provider::discover_smb_shares(&payload);
                let _=tx.send(WorkerEvent::Shares(result));
            }).map_err(|error|format!("Could not start SMB share discovery: {error}"))?;
            Ok(())
        }
        DeckFilesAction::Connect(payload) => {
            let tx = tx.clone();
            std::thread::Builder::new().name("deckfiles-smb-connect".into()).spawn(move || {
                let result = DeckFilesApp::load(&data_dir)
                    .and_then(|mut worker| worker.connect_source(&payload));
                let _ = tx.send(WorkerEvent::Connected(result));
            }).map_err(|error| format!("Could not start connection worker: {error}"))?;
            Ok(())
        }
        DeckFilesAction::ListDirectory { source_id, path } => {
            let tx = tx.clone();
            let worker_source = source_id.clone();
            let worker_path = path.clone();
            std::thread::Builder::new().name("deckfiles-directory".into()).spawn(move || {
                let result = DeckFilesApp::load(&data_dir).and_then(|mut worker| {
                    worker.list_directory(&worker_source, &worker_path)?;
                    Ok(worker.files.entries.clone())
                });
                let _ = tx.send(WorkerEvent::Directory { source_id, path, result });
            }).map_err(|error| format!("Could not start directory worker: {error}"))?;
            Ok(())
        }
        DeckFilesAction::PreparePlayback { source_id, path } => {
            let tx = tx.clone();
            std::thread::Builder::new().name("deckfiles-play-prepare".into()).spawn(move || {
                let result = DeckFilesApp::load(&data_dir)
                    .and_then(|worker| worker.prepare_play(&source_id, &path));
                let _ = tx.send(WorkerEvent::Prepared(result));
            }).map_err(|error| format!("Could not start playback worker: {error}"))?;
            Ok(())
        }
    }
}

fn show_error(app: &mut DeckFilesApp, error: String) {
    if app.page == DeckFilesPage::AddSource { app.add_source.error = Some(error); }
    else { app.files.set_error(Some(error)); }
}

fn dispatch(app: &mut DeckFilesApp, action: DeckFilesAction, data_dir: &PathBuf, tx: &Sender<WorkerEvent>) {
    match action {
        DeckFilesAction::SaveSettings => if let Err(error) = app.save_settings() { show_error(app, error); },
        DeckFilesAction::None => {},
        DeckFilesAction::DiscoverShares(_) if app.add_source.state == deckfiles_screens::add_source_modal::ConnectState::Connecting => {
            if let Err(error)=spawn_action(action,data_dir.clone(),tx) { app.finish_discover_shares(Err(error)); }
        }
        DeckFilesAction::Connect(_) if app.add_source.state == deckfiles_screens::add_source_modal::ConnectState::Connecting => {
            if let Err(error) = spawn_action(action, data_dir.clone(), tx) { app.finish_connect(Err(error)); }
        }
        other => if let Err(error) = spawn_action(other, data_dir.clone(), tx) { show_error(app, error); },
    }
}

fn update_player_snapshot(app: &mut DeckFilesApp, player: &Player, title: &str) {
    let position_ms = player::playpos_ns(&player.session).unwrap_or(0).max(0) as u64 / 1_000_000;
    let duration_ms = player::duration_ns(&player.session).map(|value| value.max(0) as u64 / 1_000_000);
    app.player.update_snapshot(PlayerSnapshot {
        title: title.to_string(), detail: "SMB direct playback".into(), position_ms, duration_ms,
        paused: !player::is_playing(&player.session), buffered_percent: 0,
        audio_tracks: Vec::new(), subtitle_tracks: Vec::new(), next_title: None, error: None,
    });
}

fn handle_key(app: &mut DeckFilesApp, key: Key, player: &mut Player, adapter: &mut PlayerAdapter) -> Option<DeckFilesAction> {
    if app.page == DeckFilesPage::Player {
        use deckfiles_screens::deck_player::{PlayerAction, PlayerKey};
        // Dedicated transport keys keep their literal meaning; only PLAY/PAUSE is a toggle.
        if key == Key::Play { let _ = player::resume(&mut player.session, adapter); app.player.snapshot.paused=false; return None; }
        if key == Key::Pause { let _ = player::pause(&mut player.session, adapter); app.player.snapshot.paused=true; return None; }
        let player_key = match key {
            Key::Up => PlayerKey::Up,
            Key::Down => PlayerKey::Down,
            Key::Left { .. } => PlayerKey::Left,
            Key::Right { .. } => PlayerKey::Right,
            Key::Ok => PlayerKey::Ok,
            Key::Back => PlayerKey::Back,
            Key::PlayPause => PlayerKey::PlayPause,
            Key::Stop => PlayerKey::Back,
            _ => return None,
        };
        if let Some(action) = app.player.handle_key(player_key) {
            match action {
                PlayerAction::Play => { let _ = player::resume(&mut player.session, adapter); app.player.snapshot.paused=false; }
                PlayerAction::Pause => { let _ = player::pause(&mut player.session, adapter); app.player.snapshot.paused=true; }
                PlayerAction::Seek(time) => {
                    let ns = (time.as_millis().min(i64::MAX as u64 / 1_000_000) as i64) * 1_000_000;
                    player::request_seek(&mut player.session, adapter, ns);
                }
                PlayerAction::Stop => {
                    player::stop_playback(&mut player.session, adapter);
                    app.page = DeckFilesPage::Files;
                    app.remote.focus_content();
                }
                _ => {}
            }
        }
        return None;
    }
    let input = match key {
        Key::Up => DeckFilesInput::Up,
        Key::Down => DeckFilesInput::Down,
        Key::Left { .. } => DeckFilesInput::Left,
        Key::Right { .. } => DeckFilesInput::Right,
        Key::Ok => DeckFilesInput::Confirm,
        Key::Back => DeckFilesInput::Back,
        Key::Play => DeckFilesInput::OpenAddSource,
        Key::Pause => DeckFilesInput::OpenSettings,
        Key::Stop if app.page == DeckFilesPage::AddSource => DeckFilesInput::ToggleGuest,
        Key::Stop => DeckFilesInput::ToggleMode,
        _ => return None,
    };
    Some(app.handle_input(input))
}

fn consume_worker_events(
    app: &mut DeckFilesApp, player: &mut Player, adapter: &mut PlayerAdapter,
    data_dir: &PathBuf, tx: &Sender<WorkerEvent>, rx: &Receiver<WorkerEvent>, current_title: &mut String,
) {
    while let Ok(event) = rx.try_recv() {
        match event {
            WorkerEvent::Shares(result) => app.finish_discover_shares(result),
            WorkerEvent::Connected(Ok(source_id)) => {
                app.finish_connect(Ok(source_id.clone()));
                match app.reload_vault(data_dir) {
                    Ok(()) if app.files.choose_source(&source_id) => {
                        app.files.set_loading(true);
                        dispatch(app, DeckFilesAction::ListDirectory { source_id, path: String::new() }, data_dir, tx);
                    }
                    Ok(()) => {}
                    Err(error) => show_error(app, error),
                }
            }
            WorkerEvent::Connected(Err(error)) => app.finish_connect(Err(error)),
            WorkerEvent::Directory { source_id, path, result } => {
                if app.files.active_source.as_deref() != Some(source_id.as_str()) { continue; }
                match result {
                    Ok(entries) => { app.files.current_path = path; app.files.set_entries(entries); }
                    Err(error) => app.files.set_error(Some(error)),
                }
            }
            WorkerEvent::Prepared(Ok(PreparedSourcePlay::Smb { prepared, .. })) => {
                *current_title = prepared.file_name.clone();
                adapter.set_external_playback(prepared.play, prepared.bridge);
                plx_media::route::clear_url(&mut player.session);
                if player::start_playback(&mut player.session, adapter) {
                    app.page = DeckFilesPage::Player;
                    update_player_snapshot(app, player, current_title);
                } else {
                    adapter.clear_external_playback();
                    app.files.set_error(Some("The TV could not start this video".into()));
                }
            }
            WorkerEvent::Prepared(Ok(PreparedSourcePlay::Local { .. })) =>
                app.files.set_error(Some("Local playback handoff is not available yet".into())),
            WorkerEvent::Prepared(Err(error)) => app.files.set_error(Some(error)),
        }
    }
}

fn process_decoded(
    decoded: Decoded, app: &mut DeckFilesApp, running: &mut bool, suspended: &mut bool,
    shell: &dyn Shell, surface: &dyn Surface, player: &mut Player, adapter: &mut PlayerAdapter,
    data_dir: &PathBuf, tx: &Sender<WorkerEvent>, text_input: &mut bool,
) {
    match decoded {
        Decoded::Quit => *running = false,
        Decoded::Key { state, wcode, sym } if state & 0xff == 1 => {
            shell.hide_pointer();
            if sym == 8 {
                let action = app.handle_input(DeckFilesInput::Backspace);
                dispatch(app, action, data_dir, tx);
                return;
            }
            // webOS AC_HOME (SDL scancode 269): hand the television its Home screen without
            // terminating DeckFiles. This is DeckFiles' own remote adapter, not the legacy app.
            if sym == 269 || wcode == Some(269) {
                plx_os::tv::home::go_home();
                return;
            }
            let key = classify(sym, wcode.unwrap_or(0));
            if key == Key::Exit { *running = false; return; }
            if let Some(action) = handle_key(app, key, player, adapter) {
                dispatch(app, action, data_dir, tx);
            }
            let wants_text = app.page == DeckFilesPage::AddSource;
            if wants_text && !*text_input {
                plx_shell_sdl::text::start_input();
                *text_input = true;
            } else if !wants_text && *text_input {
                plx_shell_sdl::text::stop_input();
                *text_input = false;
            }
        }
        Decoded::Text(value) if !value.is_empty() => {
            let action = app.handle_input(DeckFilesInput::Text(value));
            dispatch(app, action, data_dir, tx);
        }
        Decoded::Lifecycle(Lifecycle::WillBackground | Lifecycle::DidBackground) => {
            *suspended = true;
            shell.release_surface();
            plx_base::task::watchdog::set_foreground(false);
            let at = adapter.backend_tick();
            let _ = adapter.backend_set_foreground_at(&mut player.session, false, at);
        }
        Decoded::Lifecycle(Lifecycle::DidForeground) => {
            shell.grab_surface(surface.window());
            *suspended = false;
            plx_base::task::watchdog::set_foreground(true);
            let at = adapter.backend_tick();
            let _ = adapter.backend_set_foreground_at(&mut player.session, true, at);
        }
        _ => {}
    }
}

pub(crate) fn run(shell: &dyn Shell) -> Exit {
    let mt = unsafe { MainThread::assume() };
    if let Err(exit) = shell.init(&mt) { return exit; }
    let plan = shell.window(None);
    let ask = OpenAsk { title: plan.title, placement: plan.placement, high_dpi: true };
    let surface = match shell.open(&mt, &ask) { Ok(surface) => surface, Err(exit) => return exit };
    let window = surface.window();
    shell.bind_window(window);
    shell.grab_surface(window);
    shell.arm_opaque_region();
    plx_base::task::watchdog::set_foreground(true);
    surface.set_swap_interval(1);
    plx_gfx::egl::probe();
    if let Err(error) = plx_gfx::gfx::init_gl() {
        plx_base::eventlog::log(&format!("DeckFiles: graphics initialization failed: {error:?}"));
        return Exit::refused(1);
    }
    if let Err(error) = plx_gfx::text::init_text() {
        plx_base::eventlog::log(&format!("DeckFiles: text initialization failed: {error:?}"));
        return Exit::refused(1);
    }
    if let Err(error) = plx_gfx::gfx::init_image() {
        plx_base::eventlog::log(&format!("DeckFiles: image initialization failed: {error:?}"));
        return Exit::refused(1);
    }
    plx_gfx::gfx::init_blur();

    (host::WEBOS_MEDIA.early_init)(&mt);
    plx_media_codecs::ffi::boot();
    #[cfg(feature = "play-feed")]
    plx_play_feed::publish_audio_converter();
    let mut adapter = PlayerAdapter::new(mt);
    let composition = (host::WEBOS_MEDIA.compose)(&mut adapter, false);
    let mut player = Player::new();
    player.session.install_uri_preflight_port(composition.resolver);
    player.session.install_backends(adapter.composed_backends());

    // Source records and settings belong under the platform's persistent state root,
    // never beside the read-only application payload.
    let data_dir = plx_base::paths::persistent_state_root().join("DeckFiles");
    let mut app = match DeckFilesApp::load(&data_dir) {
        Ok(app) => app,
        Err(error) => {
            plx_base::eventlog::log(&format!("DeckFiles: could not load app storage: {error}"));
            return Exit::refused(1);
        }
    };
    let (tx, rx) = mpsc::channel();
    let mut running = true;
    let mut suspended = false;
    let mut text_input = false;
    let mut current_title = String::new();
    let flavor = shell.sdl().flavor;

    while running {
        shell.pump_bus();
        let mut event = [0u8; 128];
        while surface.poll(&mut event) {
            if let Some(decoded) = plx_shell_sdl::event::decode(flavor, &event) {
                process_decoded(decoded, &mut app, &mut running, &mut suspended, shell,
                    surface.as_ref(), &mut player, &mut adapter, &data_dir, &tx, &mut text_input);
            }
            if !running { break; }
        }
        consume_worker_events(&mut app, &mut player, &mut adapter, &data_dir, &tx, &rx, &mut current_title);
        if suspended { continue; }

        let now = plx_base::clock::vclock_ms();
        player.now_ms = now;
        let at = adapter.backend_tick();
        let _ = adapter.backend_set_foreground_at(&mut player.session, true, at);
        if player::is_started(&player.session) == Some(true) {
            player::pump(&mut player.session, &mut adapter, now);
        }
        let at = adapter.backend_tick();
        let _ = adapter.backend_poll_session_at(&mut player.session, at);
        if let Some(bound) = player::observe_video_plane(&mut player, &mut adapter) {
            shell.opaque_route(bound);
        }
        if app.page == DeckFilesPage::Player { update_player_snapshot(&mut app, &player, &current_title); }

        {
            #[cfg(feature = "threadcheck")]
            let _draw_scope = plx_base::task::watchdog::draw_scope();
            plx_gfx::egl::frame_damage();
            let (vx, vy, vw, vh) = plx_base::surface::viewport();
            unsafe { glViewport(vx, vy, vw, vh); }
            plx_gfx::gfx::field_frame_begin();
            plx_gfx::text::begin_frame();
            plx_gfx::gfx::straighten::frame_begin();
            if player.video_plane_bound {
                plx_gfx::gfx::frame_clear_through();
            } else {
                let (r, g, b) = plx_ui::theme::CLEAR_RGB;
                plx_gfx::gfx::frame_clear(r, g, b);
            }
            plx_ui::guard(|| app.draw(Painter::root(), Rect::new(0.0, 0.0, 1920.0, 1080.0)));
        }
        plx_gfx::gfx::straighten::straighten_surface(player.video_plane_bound);
        shell.opaque_route(player.video_plane_bound);
        (shell.frame().present.before_swap)();
        surface.swap();
    }

    plx_base::task::watchdog::set_foreground(false);
    if text_input { plx_shell_sdl::text::stop_input(); }
    if player::is_started(&player.session) == Some(true) {
        player::stop_playback(&mut player.session, &mut adapter);
    }
    shell.opaque_route(false);
    shell.clear_opaque_region();
    shell.release_surface();
    unsafe { surface.quit(); }
    Exit { code: 0, release: Release::Complete }
}
