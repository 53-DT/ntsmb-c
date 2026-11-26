//! NT-SMB connection and share-selection screen.
//!
//! Credentials remain in this live screen/form and are sent only to the SMB worker.
//! The logical digest records lengths only; password text is never serialized.
use std::{borrow::Cow, ffi::CString};
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Handled, InputKind, Key,
    LogicalState, Machine, Fx, TextEdit, Host,
};
use plx_ui::frame::Budget;
use plx_ui::cards::{AvatarRow, CardEvent, CardSource, Grid, GridSpec, RowStyle, Tile, TileLabel};
use plx_ui::geom;
use plx_ui::widgets::Art;
use plx_machine::machine::Measure;
use plx_ui::label::{Label, HAlign};
use plx_ui::consts::{CARD_W, MARGIN_X, SCR_W};
use plx_ui::screen::{
    At, AxisMask, DrawFrame, EdgeRule, ElemKind, Enter, FocusSource, Focusable, GroupKind,
    GroupSpec, HitSource, Placed, RenderStrategy, Screen, ScreenEvent, Seat, Scrim, Step, Dir,
    TextEntry, Stop,
};
use plx_ui::route_screen::RouteLayout;
use plx_ui::text_buffer::TextBuffer;
use plx_ui::text_field::{TextField, FIELD_H};
use plx_ui::widgets::Button;
use plx_ui::{theme, Env, Rect, View};
use plx_smb::{connect_form::ConnectForm, controller::{BrowseController, Phase, Activation, Back}};

const HOST: u32 = 10;
const PORT: u32 = 11;
const USER: u32 = 12;
const PASSWORD: u32 = 13;
const GUEST: u32 = 14;
const CONNECT: u32 = 15;
const SAVE: u32 = 16;
const SHARE_BASE: u32 = 100;
const SAVED_BASE: u32 = 0;
const NEW_CONNECTION: u32 = 17;
const REMOVE_CONNECTION: u32 = 20;
const CHANGE_SERVER: u32 = 18;
const PROFILE: u32 = 19;
const FORM_GROUP: plx_machine::machine::GroupId = plx_machine::machine::GroupId(0);
const PROFILE_GROUP: plx_machine::machine::GroupId = plx_machine::machine::GroupId(1);
const SAVED_FOOTER_GROUP: plx_machine::machine::GroupId = plx_machine::machine::GroupId(2);
const SAVED_ROW_Y: f32 = 384.0;
const SAVED_FOOTER_Y: f32 = 780.0;
/// Wide enough for the longest footer label ("Choose server") at BODY size without bleeding.
const SAVED_FOOTER_BTN_W: f32 = 320.0;
const GAP: f32 = theme::space::SM;
const SHARE_TOP_PAD: f32 = 112.0;
const FIELD_W: f32 = 720.0;

pub const SHAPE: &str = "SmbConnectScreen{host_len:u32,port_len:u32,user_len:u32,password_len:u32,guest:bool,save:bool,editing:u32,connecting:bool,error:bool,shares:usize}";

/// Set by the app loop when the person picks *Switch source* in Settings or the account menu. The
/// SMB page IS the root already, so a `Root(SmbConnect)` navigation is a no-op (the stack dedups
/// it); the page watches this flag instead and returns itself to the saved-sources picker.
static SHOW_SOURCES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn request_source_picker() { SHOW_SOURCES.store(true, std::sync::atomic::Ordering::Relaxed); }

pub struct SmbConnectScreen {
    pub entry: EntryId,
    pub form: ConnectForm,
    host: TextBuffer,
    port: TextBuffer,
    user: TextBuffer,
    password: TextBuffer,
    editing: Option<u32>,
    pub controller: BrowseController,
    pub shares: Vec<plx_smb::ShareInfo>,
    pub rows: Vec<plx_smb::browser::BrowserEntry>,
    pub selected: usize,
    pub error: Option<String>,
    pub connecting: bool,
    playback: Option<std::sync::mpsc::Receiver<Result<plx_smb::playback::PreparedPlayback, String>>>,
    saved: plx_smb::store::SavedStore,
    save_pending: bool,
    saved_id: Option<String>,
    last_saved_path: String,
    saved_view: bool,
    storage_warning: Option<String>,
    exit_armed: bool,
    /// True only while the person has deliberately moved up to the top-right Settings control.
    /// Without it the cursor sits on Settings whenever a folder is still loading (it is the only
    /// thing focusable) and never moves to the files once they arrive.
    profile_intent: std::sync::atomic::AtomicBool,
    saved_selection: Option<usize>,
    remove_armed: bool,
    resume_path: Option<String>,
    browser_grid: Grid,
    saved_row: AvatarRow,
}

struct SmbCardSource<'a> { rows: &'a [plx_smb::browser::BrowserEntry] }
impl<H: Host<Elem = u32>> CardSource<H> for SmbCardSource<'_> {
 fn len(&self)->usize { self.rows.len() }
 fn elem(&self,i:usize)->u32 { SHARE_BASE+i as u32 }
 fn index_of(&self,e:&u32)->Option<usize> { e.checked_sub(SHARE_BASE).map(|i|i as usize).filter(|i|*i<self.rows.len()) }
 fn art(&self,_i:usize)->Art<'_> { Art::Poster(None) }
 fn label(&self,i:usize)->TileLabel {
  // Let the shared card-label renderer marquee only the focused filename. The
  // within-tile window keeps the moving text inside this tile's own width.
  self.rows.get(i).map(|row|TileLabel::title(&row.entry.name).within_tile()).unwrap_or_default()
 }
 fn a11y_title(&self,i:usize)->Option<Cow<'_,str>> { self.rows.get(i).map(|r|Cow::Borrowed(r.entry.name.as_str())) }
 fn overlay(&self,p:plx_ui::Painter,i:usize,tile:&Tile,measure:&dyn Measure) {
  let Some(row)=self.rows.get(i) else{return};
  // Keep the icon and filename in one fixed tile. This avoids the grid's animated
  // caption band swallowing names on the first row or hiding sibling names on the
  // focused row, and it makes long names unable to bleed into neighbouring tiles.
  let glyph=CString::new(if row.is_video{"▶"}else{"📁"}).unwrap();
  let title_h=34.0;
  let title_y=tile.rect.y+tile.rect.h-title_h;
  let frame=Rect::new(tile.rect.x,tile.rect.y+8.0,tile.rect.w,tile.rect.h-title_h-12.0);
  Label::new(glyph.as_ptr(),68,if row.is_video{[0.88,0.91,0.96,1.0]}else{[0.98,0.72,0.25,1.0]}).h(HAlign::Center).draw(p,frame);
  p.rrect(Rect::new(tile.rect.x+4.0,title_y,tile.rect.w-8.0,title_h),6.0,0.0,theme::scrim_black(0.70));
  // The focused title is drawn by Grid's native marquee below the tile; drawing
  // another copy here would duplicate it. Unfocused tiles retain the compact,
  // contained caption inside their own tile.
  if !tile.focused {
   let width=(tile.rect.w-16.0).max(1.0);
   let (fitted,_)=measure.fit_line_measured(&row.entry.name,width,theme::size::LABEL,false);
   Label::new(fitted.as_ptr(),theme::size::LABEL,theme::TEXT_SECONDARY)
    .h(HAlign::Center).draw(p,Rect::new(tile.rect.x+8.0,title_y,tile.rect.w-16.0,title_h));
  }
 }
}
impl SmbConnectScreen {
    pub fn new(entry: EntryId) -> Self {
        let form = ConnectForm::new();
        // SMB profiles share FinNative's persisted session record.
        let session = plx_plex::plex::session::peek();
        let saved_payload = session.extensions.0
            .get("ntsmb_connections").and_then(serde_json::Value::as_str);
        // One-time migration from the standalone connection file used by older builds.
        let saved = if let Some(payload) = saved_payload {
            plx_smb::store::SavedStore::open_session(Some(payload))
        } else {
            let mut legacy_paths = vec![plx_base::paths::persistent_state_root().join("ntsmb-connections.json")];
            legacy_paths.extend(plx_base::paths::hud_candidates().into_iter()
                .filter(|path| path.parent() != Some(plx_base::paths::runtime_dir()))
                .map(|path| {
                    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("huds");
                    let prefix = file_name.strip_suffix("huds").unwrap_or("");
                    path.with_file_name(format!("{prefix}ntsmb-connections.json"))
                }));
            legacy_paths.push(plx_base::paths::runtime_dir().join("ntsmb-connections.json"));
            legacy_paths.dedup();
            plx_smb::store::SavedStore::open_with_runtime(
                legacy_paths, Some(plx_base::paths::runtime_dir().to_path_buf())
            )
        };
        let saved_view = true;
        let storage_warning = saved.saved_temporarily().then(|| "Connection details are stored in temporary TV storage and may be lost after a TV restart.".into());
        let saved_selection = (!saved.connections().is_empty()).then_some(0);
        Self {
            entry, host: TextBuffer::new(String::new(), 0),
            port: TextBuffer::new(form.port.clone(), form.port.len()),
            user: TextBuffer::new(String::new(), 0),
            password: TextBuffer::new(String::new(), 0),
            form, controller: BrowseController::new(),
            shares: Vec::new(), rows: Vec::new(), selected: 0, error: None, connecting: false, editing: None, playback: None,
            saved, save_pending: false, saved_id: None, last_saved_path: String::new(),
            saved_view, storage_warning, exit_armed: false, profile_intent: std::sync::atomic::AtomicBool::new(false), saved_selection, remove_armed: false, resume_path: None,
            browser_grid: Grid::new(entry, GridSpec::resting(324.0).columns(6, RowStyle { h: CARD_W * 0.75, under_tile: 72.0, gap: (SCR_W - 2.0 * MARGIN_X - 44.0 - 6.0 * CARD_W) / 5.0, ..RowStyle::HOME }, MARGIN_X)),
            saved_row: AvatarRow::new(),
        }
    }

    fn persist_saved(&self) -> Result<(), String> {
        let encoded = self.saved.encode_session()?;
        let written = plx_plex::plex::session::update(|current| {
            let mut next = current.clone();
            next.extensions.0.insert("ntsmb_connections".to_owned(), serde_json::Value::String(encoded));
            Some(next)
        });
        if written { Ok(()) } else {
            Err("Could not save SMB connections in the FinNative session. Sign in first and try again.".into())
        }
    }

    fn saved_row_geom(n: usize) -> (f32, f32) {
        let sty = AvatarRow::STYLE;
        let slot = sty.w + sty.gap;
        let total = (n as f32 * slot - sty.gap).max(0.0);
        (((SCR_W as f32 - total) * 0.5).max(sty.margin_x), slot)
    }
    fn saved_shelf(&self) -> geom::Shelf<'_> {
        self.saved_row.focusable(self.saved.connections().len(), SAVED_ROW_Y, PROFILE_GROUP, self.entry,
            Rect::new(0.0, SAVED_ROW_Y, SCR_W as f32, AvatarRow::STYLE.h))
    }
    fn buffer(&self, field: u32) -> &TextBuffer {
        match field { HOST => &self.host, PORT => &self.port, USER => &self.user, _ => &self.password }
    }
    fn buffer_mut(&mut self, field: u32) -> &mut TextBuffer {
        match field { HOST => &mut self.host, PORT => &mut self.port, USER => &mut self.user, _ => &mut self.password }
    }
    fn field(field: u32) -> bool { matches!(field, HOST | PORT | USER | PASSWORD) }
    fn text_entry(&self) -> TextEntry {
        match self.editing { Some(PASSWORD) => TextEntry::Secret, Some(_) => TextEntry::Plain, None => TextEntry::None }
    }
    fn set_editing<H: Host>(&mut self, field: Option<u32>, fx: &mut Effects<'_, H>) {
        let field = field.filter(|f| Self::field(*f));
        if self.editing == field { return; }
        if let Some(f) = field {
            let value = self.buffer(f).text().to_owned();
            *self.buffer_mut(f) = TextBuffer::new(value.clone(), value.len());
        }
        let was_up = self.editing.is_some();
        self.editing = field;
        if was_up != field.is_some() {
            let me = fx.from();
            fx.push(Fx::Deliver(me, Delivery::Keyboard { up: field.is_some() }));
        }
        fx.invalidate(plx_machine::present::Provenance::Input);
    }
    fn sync_form(&mut self) {
        self.form.host = self.host.text().to_owned();
        self.form.port = self.port.text().to_owned();
        self.form.username = self.user.text().to_owned();
        self.form.password = self.password.text().to_owned();
    }
    pub fn connect(&mut self) -> Result<(), String> {
        self.sync_form();
        let request = self.form.validate()?;
        self.controller.connect(request)?;
        if let Some(path) = self.resume_path.take() { self.controller.resume_in(&path); }
        self.connecting = true;
        self.error = None;
        self.save_pending = self.form.save_connection;
        self.last_saved_path.clear();
        self.shares.clear(); self.rows.clear(); self.selected = 0;
        Ok(())
    }
    pub fn poll(&mut self) -> Result<(), String> {
        self.controller.pump();
        self.shares = self.controller.shares().to_vec();
        let fresh = self.controller.browser().entries().to_vec();
        if fresh.len() != self.rows.len() { self.profile_intent.store(false, std::sync::atomic::Ordering::Relaxed); }
        self.rows = fresh;
        self.selected = self.controller.browser().selected();
        self.connecting = matches!(self.controller.phase(), Phase::Connecting | Phase::OpeningShare) || self.controller.browser().is_loading();
        if let Some(message) = self.controller.message() { self.error = Some(message.to_owned()); }
        if let Some(error) = self.controller.browser().error() { self.error = Some(error.to_owned()); }
        if self.save_pending && matches!(self.controller.phase(), Phase::PickShare | Phase::Browsing) {
            self.save_pending = false;
            let result = self.form.validate().and_then(|request| {
                // SMB credentials live inside the protected FinNative session auth payload.
                let secret = request.password().to_owned();
                self.saved.upsert_with_password(request.profile, (!secret.is_empty()).then_some(secret))
            });
            match result {
                Ok(id) => {
                    self.saved_id = Some(id); self.saved_view = false;
                    if let Err(error) = self.persist_saved() { self.error = Some(error); }
                    else { self.storage_warning = None; }
                }
                Err(error) => self.error = Some(format!("Connected, but could not save this server: {error}")),
            }
        }
        if let Some(id) = self.saved_id.clone() {
            let path = self.controller.folder().to_owned();
            if path != self.last_saved_path {
                self.saved.set_last_path(&id, &path);
                self.last_saved_path = path;
                if let Err(error) = self.persist_saved() { self.error = Some(error); }
            }
            if let Some(share) = self.controller.share() {
                let changed = self.saved.get(&id).is_some_and(|entry| entry.profile.share.as_deref() != Some(share));
                if changed { self.saved.set_share(&id, Some(share)); if let Err(error) = self.persist_saved() { self.error = Some(error); } }
            }
        }
        Ok(())
    }
    fn rect(field: u32, measure: &dyn plx_machine::machine::Measure) -> Option<Rect> {
        let layout = RouteLayout::screen();
        let width = layout.content.w.min(FIELD_W);
        let total = 4.0 * FIELD_H + 4.0 * GAP + layout.action.h;
        let x = layout.content.x + (layout.content.w - width) * 0.5;
        let top = (layout.content.cy() - total * 0.5).max(layout.content.y);
        let field_rect = |i: f32| Rect::new(x, top + i * (FIELD_H + GAP), width, FIELD_H);
        let button_y = top + 4.0 * (FIELD_H + GAP);
        let button_w = Button::pill_w_measured(plx_os::i18n::msg::browse_action_sign_in_c(), theme::size::BODY, false, None, measure);
        match field {
            HOST => Some(field_rect(0.0)), PORT => Some(field_rect(1.0)),
            USER => Some(field_rect(2.0)), PASSWORD => Some(field_rect(3.0)),
            CONNECT => Some(Rect::new(x, button_y, button_w, layout.action.h)),
            GUEST => Some(Rect::new(x + button_w + theme::space::MD, button_y, button_w, layout.action.h)),
            SAVE => Some(Rect::new(x + 2.0 * (button_w + theme::space::MD), button_y, button_w, layout.action.h)),
            _ => None,
        }
    }
    fn stop_rect(&self, field: u32, measure: &dyn plx_machine::machine::Measure) -> Option<Rect> {
        if field == NEW_CONNECTION || field == REMOVE_CONNECTION {
            return Self::saved_footer_rect(field);
        }
        if field == CHANGE_SERVER {
            let base=Self::rect(HOST,measure)?;
            return Some(Rect::new(base.x,base.y+SHARE_TOP_PAD+self.shares.len() as f32*(FIELD_H+GAP),base.w,FIELD_H));
        }
        if field >= SHARE_BASE && field < SHARE_BASE+self.shares.len() as u32 && self.controller.phase()!=Phase::Browsing {
            let i=(field-SHARE_BASE) as usize;
            let base=Self::rect(HOST,measure)?;
            return Some(Rect::new(base.x,base.y+SHARE_TOP_PAD+i as f32*(FIELD_H+GAP),base.w,FIELD_H));
        }
        if field == PROFILE { return Some(Self::profile_rect()); }
        let r = Self::rect(field, measure)?;
        Some(if Self::field(field) { plx_ui::text_field::plate(r) } else { r })
    }
    fn activate_browser_index(&mut self, index: usize) {
        if index >= self.rows.len() { return; }
        if self.rows[index].is_video {
            let request = self.controller.request().cloned();
            let share = self.controller.share().map(str::to_owned);
            let folder = self.controller.folder().to_owned();
            let queue = match (request, share) {
                (Some(request), Some(share)) => self.rows.iter().enumerate()
                    .filter(|(_, row)| row.is_video)
                    .filter_map(|(row_index, row)| {
                        plx_smb::browser::child_path(&folder, &row.entry.name).ok().map(|path| {
                            (row_index, plx_smb::controller::PlayRequest { request: request.clone(), share: share.clone(), path })
                        })
                    }).collect(),
                _ => Vec::new(),
            };
            plx_smb::playback::set_autoplay_queue(queue, index);
        } else {
            plx_smb::playback::clear_autoplay_queue();
        }
        let current = self.controller.browser().selected();
        self.controller.move_selection(index as isize - current as isize);
        self.selected = index;
        match self.controller.activate() {
            Activation::None => {}
            Activation::Play(play) => {
                if self.playback.is_some() {
                    self.error = Some("A video is already being prepared".into());
                } else {
                    match plx_smb::controller::spawn_prepare(play) {
                        Ok(rx) => { self.playback = Some(rx); self.connecting = true; self.error = None; }
                        Err(error) => { self.error = Some(error); self.connecting = false; }
                    }
                }
            }
        }
    }
    fn activate_element<H: crate::registry::AppLike<Memory = crate::registry::PageMemory>>(&mut self, elem: u32, fx: &mut Effects<'_, H>) {
        self.exit_armed=false;
        match elem {
            REMOVE_CONNECTION => {
                self.remove_armed = !self.remove_armed;
                self.error = self.remove_armed.then(|| "Choose the server you want to remove.".into());
                fx.invalidate(plx_machine::present::Provenance::Input);
            }
            PROFILE => {
                fx.push(Fx::App(crate::registry::AppFx::Loop(
                    crate::registry::LoopReq::OpenAccountMenu,
                )));
            }
            HOST|PORT|USER|PASSWORD => self.set_editing(if self.editing==Some(elem){None}else{Some(elem)},fx),
            GUEST => { self.form.guest=!self.form.guest; if self.form.guest { self.set_editing(None,fx); } fx.invalidate(plx_machine::present::Provenance::Input); }
            SAVE => { self.form.save_connection=!self.form.save_connection; fx.invalidate(plx_machine::present::Provenance::Input); }
            NEW_CONNECTION => { self.saved_view=false; self.saved_id=None; self.exit_armed=false; self.remove_armed=false; self.error=None; self.host=TextBuffer::new(String::new(),0); self.port=TextBuffer::new("445".into(),3); self.user=TextBuffer::new(String::new(),0); self.password=TextBuffer::new(String::new(),0); self.form=ConnectForm::new(); self.set_editing(None,fx); fx.invalidate(plx_machine::present::Provenance::Input); }
            CHANGE_SERVER => { self.controller.disconnect(); self.shares.clear(); self.rows.clear(); self.selected=0; self.connecting=false; self.save_pending=false; self.saved_id=None; self.saved_view=true; self.error=None; self.set_editing(None,fx); fx.invalidate(plx_machine::present::Provenance::Input); }
            e if self.saved_view && (e as usize)<self.saved.connections().len() => {
                let index=(e-SAVED_BASE) as usize;
                if self.remove_armed {
                    if let Some(connection)=self.saved.connections().get(index) {
                        let id=connection.id.clone();
                        self.saved.remove(&id);
                        self.saved_selection=if self.saved.connections().is_empty(){None}else{Some(index.min(self.saved.connections().len()-1))};
                        self.saved_id=None;
                        self.remove_armed=false;
                        self.error=None;
                        if let Err(error)=self.persist_saved(){self.error=Some(error);}
                        fx.invalidate(plx_machine::present::Provenance::Input);
                    }
                    return;
                }
                if let Some(saved)=self.saved.connections().get(index).cloned(){
                    self.host=TextBuffer::new(saved.profile.host.clone(),saved.profile.host.len());
                    self.port=TextBuffer::new(saved.profile.port.to_string(),saved.profile.port.to_string().len());
                    self.user=TextBuffer::new(saved.profile.username.clone(),saved.profile.username.len());
                    let password=saved.password.clone().or_else(|| saved.profile.reveal_password()).unwrap_or_default();
                    self.password=TextBuffer::new(password.clone(),password.len());
                    self.form.host=saved.profile.host.clone(); self.form.port=saved.profile.port.to_string();
                    self.form.username=saved.profile.username.clone(); self.form.password=password;
                    self.form.guest=saved.profile.guest; self.form.share=saved.profile.share.clone();
                    self.form.save_connection=true; self.saved_id=Some(saved.id.clone());
                    self.saved.mark_used(&saved.id); let _=self.persist_saved();
                    self.resume_path=Some(saved.last_path.clone());
                    self.saved_view=false;
                    if !saved.profile.guest && self.form.password.is_empty(){self.error=Some("Enter the password for this saved server".into());self.set_editing(Some(PASSWORD),fx);}
                    else if let Err(error)=self.connect(){self.error=Some(error);}
                    fx.invalidate(plx_machine::present::Provenance::Input);
                }
            }
            CONNECT => { self.set_editing(None,fx); if let Err(e)=self.connect(){self.error=Some(e);} }
            e if e>=SHARE_BASE && !self.shares.is_empty() && self.controller.phase()!=Phase::Browsing => { let i=(e-SHARE_BASE) as usize; if let Some(share)=self.shares.get(i){ self.controller.select_share(&share.name); self.connecting=true; self.error=None; } }
            e if e>=SHARE_BASE && self.controller.phase()==Phase::Browsing => self.activate_browser_index((e-SHARE_BASE) as usize),
            _=>{}
        }
    }
    fn ids(&self) -> Vec<u32> {
        if self.controller.phase()==Phase::Browsing {
            let rows: Vec<u32> = (0..self.rows.len()).map(|i| SHARE_BASE+i as u32).collect();
            if rows.is_empty() { Vec::new() } else { rows }
        } else if !self.shares.is_empty() {
            (0..self.shares.len()).map(|i| SHARE_BASE + i as u32).chain(std::iter::once(CHANGE_SERVER)).collect()
        } else if self.saved_view {
            (0..self.saved.connections().len()).map(|i| SAVED_BASE+i as u32).chain([NEW_CONNECTION, REMOVE_CONNECTION]).collect()
        } else {
            vec![HOST, PORT, USER, PASSWORD, CONNECT, GUEST, SAVE]
        }
    }
    fn draw_saved_name(p: plx_ui::Painter, name: &str, cx: f32, focused: bool, measure: &dyn Measure) {
        let color = if focused { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
        let elided = plx_gfx::text::elide_by(
            name,
            AvatarRow::STYLE.w + AvatarRow::STYLE.gap - 12.0,
            false,
            |text| measure.width_str(text, theme::size::LABEL, focused),
        );
        if let Ok(label) = CString::new(elided) {
            let name_dy = AvatarRow::STYLE.h
                + AvatarRow::STYLE.h * (AvatarRow::STYLE.focus_scale - 1.0) * 0.5
                + theme::space::MD;
            p.text(label.as_ptr(), cx, SAVED_ROW_Y + name_dy, theme::size::LABEL, color, 1, if focused { 1 } else { 0 });
        }
    }
    /// The small top-left source chip (FinNative's profile chip, as *Switch source* / *Settings*).
    fn profile_rect() -> Rect { Rect::new(MARGIN_X, 40.0, 96.0, 56.0) }
    fn saved_footer_rect(elem: u32) -> Option<Rect> {
        let width = SAVED_FOOTER_BTN_W;
        let gap = 24.0;
        let total = width * 2.0 + gap;
        let x = (SCR_W as f32 - total) * 0.5;
        let index = match elem { NEW_CONNECTION => 0.0, REMOVE_CONNECTION => 1.0, _ => return None };
        Some(Rect::new(x + index * (width + gap), SAVED_FOOTER_Y, width, 60.0))
    }
    /// The orb now lives in the shared bar (`app::bridge::draw_chrome`), not on this page.
    fn draw_profile_controls(&self, _p: plx_ui::Painter, _measure: &dyn Measure, _focus: Option<u32>) {}
}
impl LogicalState for SmbConnectScreen {
 fn write(&self,c:&mut Canon){
  c.u32(self.host.text().len() as u32).u32(self.port.text().len() as u32)
   .u32(self.user.text().len() as u32).u32(self.password.text().len() as u32)
   .bool(self.form.guest).bool(self.form.save_connection).u32(self.editing.unwrap_or(0)).bool(self.connecting)
   .bool(self.error.is_some()).bool(self.storage_warning.is_some()).bool(self.exit_armed).bool(self.remove_armed).seq(self.shares.len()).seq(self.rows.len()).u32(self.selected as u32);
 }
 fn probe(&self,out:&mut String){out.push_str("smb_connect");}
}
impl<H:crate::registry::AppLike<Memory=crate::registry::PageMemory>> Machine<H> for SmbConnectScreen {
 type Ev=ScreenEvent<H>;
 fn step(&mut self,ev:&Self::Ev,_cx:&Cx<'_,H>,fx:&mut Effects<'_,H>)->Handled {
  if self.controller.phase()==Phase::Browsing {
   let src=SmbCardSource{rows:&self.rows};
   if let Some(CardEvent::Activate(elem))=self.browser_grid.on(ev,_cx,&src,fx) {
    if elem>=SHARE_BASE { self.activate_browser_index((elem-SHARE_BASE) as usize); }
    return Handled::Yes;
   }
  }
  match ev {
   ScreenEvent::Enter(Enter::Restored) => {
    if let Some((index, play)) = plx_smb::playback::take_autoplay_next() {
     let current=self.controller.browser().selected();
     self.controller.move_selection(index as isize-current as isize);
     self.selected=index;
     match plx_smb::controller::spawn_prepare(play) {
      Ok(rx)=>{self.playback=Some(rx);self.connecting=true;self.error=None;}
      Err(error)=>{self.error=Some(error);self.connecting=false;plx_smb::playback::clear_autoplay_queue();}
     }
    }
    Handled::Yes
   }
   ScreenEvent::Tick(t)=>{
    let focus=_cx.focus.current.map(|key|key.elem);
    let saved_focus = if self.saved_view { focus.filter(|e| *e >= SAVED_BASE && (*e-SAVED_BASE) < self.saved.connections().len() as u32).map(|e| (e-SAVED_BASE) as usize) } else { None };
    self.saved_row.set_margin_x(Self::saved_row_geom(self.saved.connections().len()).0);
    self.saved_row.update(self.saved.connections().len(), saved_focus, t.dt());
    if let Err(error)=self.poll(){self.error=Some(error);self.connecting=false;}
    if SHOW_SOURCES.swap(false, std::sync::atomic::Ordering::Relaxed) {
     // "Switch source": back to the saved-connections picker, from wherever we are browsing.
     self.controller.disconnect();self.shares.clear();self.rows.clear();self.selected=0;self.connecting=false;self.save_pending=false;self.saved_view=true;self.saved_id=None;self.remove_armed=false;self.exit_armed=false;self.error=None;self.set_editing(None,fx);fx.invalidate(plx_machine::present::Provenance::Input);
    }
    if let Some(rx)=self.playback.as_ref(){
     match rx.try_recv(){
      Ok(Ok(prepared))=>{self.playback=None;self.connecting=false;fx.push(Fx::App(crate::registry::AppFx::SmbPlaybackReady(prepared)));}
      Ok(Err(error))=>{self.playback=None;self.connecting=false;self.error=Some(format!("Could not prepare video: {error}"));}
      Err(std::sync::mpsc::TryRecvError::Empty)=>{self.connecting=true;}
      Err(std::sync::mpsc::TryRecvError::Disconnected)=>{self.playback=None;self.connecting=false;self.error=Some("The video preparation worker stopped unexpectedly".into());}
     }
    }
    Handled::Yes
   }
   ScreenEvent::Activate(elem) => {
     self.activate_element(*elem,fx);
     Handled::Yes
    }
    // Card and Control elements commit through the press machine, not Activate.
    // The profile avatars and Add Connection footer are both such elements.
    ScreenEvent::PressCommit(_) => {
     if let Some(key)=_cx.focus.current {
      // The footer has immediate input paths below. Do not replay its deferred commit after
      // one of those paths has already left the picker.
      if !(self.saved_view && matches!(key.elem,NEW_CONNECTION|REMOVE_CONNECTION)) { self.activate_element(key.elem,fx); }
     }
     Handled::Yes
    }
    ScreenEvent::Input(input)=>match &input.kind {
    // Pointer clicks carry their hit target directly; pointer focus need not move before commit.
    InputKind::Click{hit:Some(elem),..} if self.saved_view && matches!(*elem,NEW_CONNECTION|REMOVE_CONNECTION)=>{
     self.activate_element(*elem,fx);
     Handled::Yes
    }
    // Enter the footer on Magic Remote OK-down instead of depending on a deferred PressCommit
    // whose focus snapshot may still refer to the previous roster element.
    InputKind::Key{key:Key::Ok,edge:Edge::Down,..}
     if self.saved_view && _cx.focus.current.is_some_and(|key|key.entry==self.entry && matches!(key.elem,NEW_CONNECTION|REMOVE_CONNECTION))=>{
     if let Some(key)=_cx.focus.current { self.activate_element(key.elem,fx); }
     Handled::Yes
    }
    InputKind::SystemKeyboard(up)=>{if !*up {self.editing=None;} Handled::Yes}
    InputKind::Text(edit)=>{if let Some(field)=self.editing {self.buffer_mut(field).edit(edit);self.sync_form();fx.invalidate(plx_machine::present::Provenance::Input);} Handled::Yes}
    InputKind::Key{key,sym,edge,..} if *edge!=Edge::Up=>{
      if *key!=Key::Back && self.exit_armed { self.exit_armed=false; }
      if let Some(field)=self.editing {
       let edit=match (*key,*sym) {(Key::Left,_)=>Some(TextEdit::Left),(Key::Right,_)=>Some(TextEdit::Right),(_,plx_ui::consts::SDLK_BACKSPACE)=>Some(TextEdit::Backspace),(_,plx_ui::consts::SDLK_CLEAR)=>Some(TextEdit::Clear),_=>None};
       if let Some(edit)=edit {self.buffer_mut(field).edit(&edit);self.sync_form();fx.invalidate(plx_machine::present::Provenance::Input);return Handled::Yes;}
       if matches!(key,Key::Back|Key::Ok){self.set_editing(None,fx);return Handled::Yes;}
       if matches!(key,Key::Up|Key::Down){self.set_editing(None,fx);return Handled::No;}
       return Handled::Yes;
      }
      if *key==Key::Back {
       if self.saved_view {
        if self.exit_armed {
         self.exit_armed=false;
         fx.push(Fx::App(crate::registry::AppFx::Loop(crate::registry::LoopReq::BackAtRoot)));
        } else {
         self.exit_armed=true;
         fx.invalidate(plx_machine::present::Provenance::Input);
        }
        Handled::Yes
       } else if self.controller.phase()==Phase::Browsing {
        match self.controller.go_back(){
         Back::Handled=>{self.error=None;self.connecting=true;},
         Back::Leave=>{self.controller.disconnect();self.shares.clear();self.rows.clear();self.connecting=false;self.saved_view=true;self.saved_id=None;self.exit_armed=false;self.error=None;}
        }
        Handled::Yes
       } else if !self.shares.is_empty() {
        self.controller.disconnect();self.shares.clear();self.rows.clear();self.connecting=false;self.saved_view=true;self.saved_id=None;self.exit_armed=false;self.error=None;fx.invalidate(plx_machine::present::Provenance::Input);Handled::Yes
       } else {
        self.saved_view=true;self.exit_armed=false;self.set_editing(None,fx);fx.invalidate(plx_machine::present::Provenance::Input);Handled::Yes
       }
      } else if matches!(key,Key::Up|Key::Down|Key::Left|Key::Right) { Handled::No } else {Handled::No}
    }
    _=>Handled::No
   },
   ScreenEvent::FocusMoved{to,..}=>{
    if self.editing.is_some_and(|e|e!=to.elem){self.set_editing(None,fx);}
    if self.saved_view && (to.elem as usize)<self.saved.connections().len() {
     self.saved_selection=Some(to.elem as usize);
    }
    if self.exit_armed { self.exit_armed=false; }
    if self.controller.phase()==Phase::Browsing && to.elem>=SHARE_BASE {let next=(to.elem-SHARE_BASE) as usize;let delta=next as isize-self.controller.browser().selected() as isize;self.controller.move_selection(delta);self.selected=next;}
    Handled::Yes
   }
   ScreenEvent::WillLeave(_)|ScreenEvent::Cover|ScreenEvent::Suspend|ScreenEvent::Unmount=>{self.set_editing(None,fx);Handled::No}
   _=>Handled::No
  }
 }
}
impl<H:crate::registry::AppLike<Memory=crate::registry::PageMemory>> Focusable<H> for SmbConnectScreen {
 fn groups(&self,cx:&Cx<'_,H>,out:&mut Vec<GroupSpec>){
  let ids=self.ids(); if ids.is_empty(){return;}
  if self.saved_view {
   if !self.saved.connections().is_empty() { Focusable::<H>::groups(&self.saved_shelf(),cx,out); }
   let r=Rect::new((SCR_W as f32-(SAVED_FOOTER_BTN_W*2.0+24.0))*0.5,SAVED_FOOTER_Y,SAVED_FOOTER_BTN_W*2.0+24.0,60.0);
   out.push(GroupSpec{id:SAVED_FOOTER_GROUP,kind:GroupKind::Row{wrap:false},seat:Seat::First,reachable:AxisMask::VERTICAL,edge:[EdgeRule::Geometric,EdgeRule::Stop,EdgeRule::Stop,EdgeRule::Stop],extent:r,len:2,elem:ElemKind::Control});
   return;
  }
  if self.controller.phase()==Phase::Browsing && !self.rows.is_empty() {
   out.push(GroupSpec{id:FORM_GROUP,kind:GroupKind::Grid{cols:6,holes:&[]},seat:Seat::First,reachable:AxisMask::BOTH,edge:[EdgeRule::Geometric;4],extent:Rect::new(MARGIN_X,324.0,SCR_W-2.0*MARGIN_X,CARD_W*0.75),len:self.rows.len(),elem:ElemKind::Card});
  } else {
   let extent=ids.iter().filter_map(|e|self.stop_rect(*e,cx.measure)).reduce(|a,b|a.union(b)).unwrap_or(Rect::FULL);
   out.push(GroupSpec{id:FORM_GROUP,kind:GroupKind::Free,seat:Seat::First,reachable:AxisMask::BOTH,edge:[EdgeRule::Geometric,EdgeRule::Stop,EdgeRule::Stop,EdgeRule::Stop],extent,len:ids.len(),elem:ElemKind::Bare});
  }
 }
 fn group_of(&self,elem:&u32,cx:&Cx<'_,H>)->Option<plx_machine::machine::GroupId>{if self.saved_view { if matches!(*elem,NEW_CONNECTION|REMOVE_CONNECTION) {Some(SAVED_FOOTER_GROUP)} else {Focusable::<H>::group_of(&self.saved_shelf(),elem,cx)} } else if self.controller.phase()==Phase::Browsing && !self.rows.is_empty() && *elem==PROFILE {Some(PROFILE_GROUP)} else {self.ids().contains(elem).then_some(FORM_GROUP)}}
 fn neighbour(&self,key:FocusKey<u32>,dir:Dir,cx:&Cx<'_,H>)->Step<u32>{
  if self.saved_view {
   match (key.elem,dir) {
    (NEW_CONNECTION,Dir::Right)=>return Step::Move(FocusKey{entry:self.entry,elem:REMOVE_CONNECTION}),
    (REMOVE_CONNECTION,Dir::Left)=>return Step::Move(FocusKey{entry:self.entry,elem:NEW_CONNECTION}),
    (NEW_CONNECTION|REMOVE_CONNECTION,_)=>return Step::Edge,
    _=>return Focusable::<H>::neighbour(&self.saved_shelf(),key,dir,cx),
   }
  }
  if self.controller.phase()==Phase::Browsing {
   let src=SmbCardSource{rows:&self.rows}; return self.browser_grid.neighbour::<H,_>(&src,key,dir);
  }
  if !self.shares.is_empty() {
   let ids: Vec<u32> = (0..self.shares.len()).map(|i|SHARE_BASE+i as u32).chain(std::iter::once(CHANGE_SERVER)).collect();
   if let Some(index)=ids.iter().position(|id|*id==key.elem) {
    let target=match dir { Dir::Up=>index.checked_sub(1), Dir::Down=>(index+1<ids.len()).then_some(index+1), _=>None };
    if let Some(next)=target { return Step::Move(FocusKey{entry:self.entry,elem:ids[next]}); }
   }
   return Step::Edge;
  }
  // The three actions share one horizontal row. Declare their D-pad links explicitly
  // before the general geometric search so remote navigation follows their actual layout.
  let horizontal = match (key.elem, dir) {
   (CONNECT, Dir::Right) => Some(GUEST),
   (GUEST, Dir::Left) => Some(CONNECT),
   (GUEST, Dir::Right) => Some(SAVE),
   (SAVE, Dir::Left) => Some(GUEST),
   _ => None,
  };
  if let Some(elem)=horizontal.filter(|elem|self.ids().contains(elem)) {
   return Step::Move(FocusKey{entry:self.entry,elem});
  }
  let ids=self.ids();
  let Some(from)=self.stop_rect(key.elem,cx.measure) else{return Step::Edge};
  let mut best:Option<(f32,u32)>=None;
  for candidate in ids.into_iter().filter(|id|*id!=key.elem) {
   let Some(to)=self.stop_rect(candidate,cx.measure) else{continue};
   let (primary,orthogonal)=match dir {
    Dir::Up=>(from.y-(to.y+to.h),(from.cx()-to.cx()).abs()),
    Dir::Down=>(to.y-(from.y+from.h),(from.cx()-to.cx()).abs()),
    Dir::Left=>(from.x-(to.x+to.w),(from.cy()-to.cy()).abs()),
    Dir::Right=>(to.x-(from.x+from.w),(from.cy()-to.cy()).abs()),
   };
   // Directional, situation-aware D-pad movement: only candidates genuinely
   // in the pressed half-plane qualify; nearest axis distance wins, with a
   // smaller penalty for horizontal/vertical misalignment.
   if primary < -1.0 {continue;}
   let score=primary.max(0.0)*3.0+orthogonal;
   if best.is_none_or(|(old,_)|score<old){best=Some((score,candidate));}
  }
  best.map_or(Step::Edge,|(_,elem)|Step::Move(FocusKey{entry:self.entry,elem}))
 }
 fn place(&self,elem:&u32,cx:&Cx<'_,H>,at:At)->Option<Placed>{
  if self.saved_view { if matches!(*elem,NEW_CONNECTION|REMOVE_CONNECTION) {let r=Self::saved_footer_rect(*elem)?;return Some(Placed{rect:r,rest_rect:r,clip:Rect::FULL,index:Some(if *elem==NEW_CONNECTION{0}else{1})});} return Focusable::<H>::place(&self.saved_shelf(),elem,cx,at); }
  if *elem==PROFILE { let r=Self::profile_rect(); return Some(Placed{rect:r,rest_rect:r,clip:Rect::FULL,index:Some(0)}); }
  if self.controller.phase()==Phase::Browsing && *elem>=SHARE_BASE { let src=SmbCardSource{rows:&self.rows}; return self.browser_grid.place(cx,&src,elem,at); }
  let r=if *elem>=SHARE_BASE && *elem<SHARE_BASE+self.shares.len() as u32 {let i=(*elem-SHARE_BASE) as usize;let base=Self::rect(HOST,cx.measure)?;Some(Rect::new(base.x,base.y+SHARE_TOP_PAD+i as f32*(FIELD_H+GAP),base.w,FIELD_H))}
  else if *elem==NEW_CONNECTION {let layout=RouteLayout::screen();Some(Rect::new(layout.content.x,layout.content.y+self.saved.connections().len() as f32*(FIELD_H+GAP),layout.content.w.min(FIELD_W),FIELD_H))}
  else if *elem==CHANGE_SERVER {let base=Self::rect(HOST,cx.measure)?;Some(Rect::new(base.x,base.y+SHARE_TOP_PAD+self.shares.len() as f32*(FIELD_H+GAP),base.w,FIELD_H))}
  else{self.stop_rect(*elem,cx.measure)}?;
  Some(Placed{rect:r,rest_rect:r,clip:Rect::FULL,index:Some(self.ids().iter().position(|e|e==elem).unwrap_or(0) as u32)})
 }
 fn reconcile(&self,want:FocusKey<u32>,cx:&Cx<'_,H>)->FocusKey<u32>{if self.saved_view { if matches!(want.elem,NEW_CONNECTION|REMOVE_CONNECTION) {want} else if self.saved.connections().is_empty(){FocusKey{entry:self.entry,elem:NEW_CONNECTION}} else {Focusable::<H>::reconcile(&self.saved_shelf(),want,cx)} } else if self.ids().contains(&want.elem){want}else{FocusKey{entry:self.entry,elem:self.ids().first().copied().unwrap_or(HOST)}}}
 fn seat(&self,g:plx_machine::machine::GroupId,from:Placed,cx:&Cx<'_,H>)->FocusKey<u32>{
  if self.saved_view { if g==SAVED_FOOTER_GROUP {return FocusKey{entry:self.entry,elem:NEW_CONNECTION};} return Focusable::<H>::seat(&self.saved_shelf(),g,from,cx); }
  if g==PROFILE_GROUP { return FocusKey{entry:self.entry,elem:PROFILE}; }
  if self.controller.phase()==Phase::Browsing { let src=SmbCardSource{rows:&self.rows}; if let Some(key)=self.browser_grid.seat::<H,SmbCardSource<'_>>(&src,from){return key;} }
  FocusKey{entry:self.entry,elem:self.ids().first().copied().unwrap_or(HOST)}
 }
}
impl<H:crate::registry::AppLike<Memory=crate::registry::PageMemory>> Screen<H> for SmbConnectScreen {
 fn name(&self)->&'static str{"smb_connect"}
 fn state(&self)->&dyn LogicalState{self}
 fn crumb(&self,_cx:&Cx<'_,H>)->Option<Cow<'_,str>>{None}
 fn prepare(&mut self,_b:&mut Budget,_cx:&Cx<'_,H>){}
 fn scrim(&self)->Scrim{Scrim::NONE}
 fn draw(&mut self,f:&mut DrawFrame<'_, '_,H>){
  let p=f.painter; let layout=RouteLayout::screen(); let measure=f.measure;
  let focus=f.focus.current.map(|k|k.elem);
  // Each screen state owns ONE header. Drawing the generic connect narrative first and
  // the browser/share narrative second caused both to occupy the same top-left area.
  if self.controller.phase()!=Phase::Browsing && !self.saved_view && self.shares.is_empty() {
   let body=if self.connecting{"Connecting to the SMB server and loading shares…".to_owned()}
    else if let Some(error)=self.error.as_deref(){format!("Connection failed: {error}")}
    else{"Enter your server details to browse and play videos.".to_owned()};
   layout.draw_narrative(p,None,"Connect to SMB",&body,measure);
  }
  if self.saved_view {
   let help=if self.exit_armed {"Press Back again to return to webOS Home."} else {self.error.as_deref().or(self.storage_warning.as_deref()).unwrap_or("Choose a saved server or add a new connection.")};
   layout.draw_narrative(p,None,"Who's watching?",help,measure);
   let users=self.saved.connections(); let n=users.len();
   let (start_x,slot)=Self::saved_row_geom(n); let scroll=self.saved_row.scroll_x();
   let extent=Rect::new(0.0,SAVED_ROW_Y,SCR_W as f32,AvatarRow::STYLE.h);
   let focused_i=focus.filter(|e|(*e as usize)<n).map(|e|e as usize);
   let active_i=focused_i.or(self.saved_selection.filter(|i|*i<n));
   for (i,saved) in users.iter().enumerate() {
    let active=active_i==Some(i);
    let cx=start_x+i as f32*slot+AvatarRow::STYLE.w*0.5-scroll;
    let base=Rect::new(cx-AvatarRow::STYLE.w*0.5,SAVED_ROW_Y,AvatarRow::STYLE.w,AvatarRow::STYLE.h);
    let key=FocusKey{entry:self.entry,elem:i as u32};
    let rect=self.saved_row.draw(p,i,base,Art::Poster(None),active,measure,&f.press,&key);
    let mark=CString::new("SMB").unwrap();
    Label::new(mark.as_ptr(),46,if active{theme::ACCENT_INK}else{theme::TEXT_PRIMARY}).h(HAlign::Center).draw(p,Rect::new(rect.x,rect.y+rect.h*0.34,rect.w,48.0));
    Self::draw_saved_name(p,&saved.name,cx,active,measure);
    f.stop(p,Stop{key,rect,rest_rect:base,clip:extent,hover:plx_ui::screen::Hover::Focus,activate:plx_ui::screen::Activate::Press});
   }
   let add=CString::new("Add connection").unwrap();
   let add_r=Self::saved_footer_rect(NEW_CONNECTION).unwrap();
   Button::new(add.as_ptr(),theme::size::BODY,add_r).focused(focus==Some(NEW_CONNECTION)).draw(&Env::inert(),p);
   f.stop(p,Stop{key:FocusKey{entry:self.entry,elem:NEW_CONNECTION},rect:add_r,rest_rect:add_r,clip:Rect::FULL,hover:plx_ui::screen::Hover::Focus,activate:plx_ui::screen::Activate::Press});
   let remove=CString::new(if self.remove_armed{"Choose server"}else{"Remove selected"}).unwrap();
   let remove_r=Self::saved_footer_rect(REMOVE_CONNECTION).unwrap();
   Button::new(remove.as_ptr(),theme::size::BODY,remove_r).focused(focus==Some(REMOVE_CONNECTION)).draw(&Env::inert(),p);
   f.stop(p,Stop{key:FocusKey{entry:self.entry,elem:REMOVE_CONNECTION},rect:remove_r,rest_rect:remove_r,clip:Rect::FULL,hover:plx_ui::screen::Hover::Focus,activate:plx_ui::screen::Activate::Press});
   return;
  }
  if self.controller.phase()==Phase::Browsing {
   let browser_copy=self.controller.location();
   layout.draw_narrative(p.translate(0.0, -self.browser_grid.scroll()),None,"SMB Browser",&browser_copy,measure);
   let src=SmbCardSource{rows:&self.rows};
   self.browser_grid.draw(f,p.clipped(Rect::new(MARGIN_X,300.0,SCR_W-2.0*MARGIN_X,plx_ui::consts::SCR_H-300.0)),&src);
   self.draw_profile_controls(p,measure,focus);
   return;
  }
  if !self.shares.is_empty(){
   // Keep save/credential-store failures visible on the share picker instead of
   // silently hiding them behind the early-returning share list.
   if let Some(error)=self.error.as_deref(){
    layout.draw_narrative(p,None,"SMB Shares",error,measure);
   } else {
    layout.draw_narrative(p,None,"SMB Shares","Select a share to browse its folders.",measure);
   }
   for (i,share) in self.shares.iter().enumerate(){
    let id=SHARE_BASE+i as u32; let Some(r)=Self::rect(HOST,measure).map(|r|Rect::new(r.x,r.y+SHARE_TOP_PAD+i as f32*(FIELD_H+GAP),r.w,FIELD_H)) else{continue};
    TextField::new(plx_os::i18n::msg::browse_jf_server_c(),&share.name).hot(if focus==Some(id){1.0}else{0.0}).draw(p,r,measure);
   }
   let share_base=Self::rect(HOST,measure).unwrap_or(layout.content);
   let change_y=share_base.y+SHARE_TOP_PAD+self.shares.len() as f32*(FIELD_H+GAP);
   let change_r=Rect::new(share_base.x,change_y,share_base.w,FIELD_H);
   Button::new(CString::new("Change server / IP").unwrap().as_ptr(),theme::size::BODY,change_r).focused(focus==Some(CHANGE_SERVER)).draw(&Env::inert(),p);
   self.draw_profile_controls(p,measure,focus);
   return;
  }
  let fields=[(HOST,"Server / IP",&self.host,false),(PORT,"Port",&self.port,false),(USER,"Username",&self.user,false),(PASSWORD,"Password",&self.password,true)];
  for (id,label,buffer,secret) in fields{
   if let Some(r)=Self::rect(id,measure){
    let label=CString::new(label).expect("static field label");
    TextField::new(label.as_c_str(),buffer.text()).caret(buffer.caret()).secret(secret).hot(if focus==Some(id){1.0}else{0.0}).editing(self.editing==Some(id),true).draw(p,r,measure);
   }
  }
  for (id,label) in [(CONNECT,"Connect"),(GUEST,if self.form.guest{"Guest: On"}else{"Guest: Off"}),(SAVE,if self.form.save_connection{"Save: On"}else{"Save: Off"})]{
   if let Some(r)=Self::rect(id,measure){
    let label=CString::new(label).expect("static button label");
    Button::new(label.as_ptr(),theme::size::BODY,r).focused(focus==Some(id)).draw(&Env::inert(),p);
   }
  }
  self.draw_profile_controls(p,measure,focus);
 }
 fn render(&self)->RenderStrategy{RenderStrategy::Page}
 fn focus_source(&self)->FocusSource{FocusSource::Engine}
 fn hit_source(&self)->HitSource{HitSource::Engine}
 fn text_entry(&self)->TextEntry{self.text_entry()}
 fn as_any(&self)->Option<&dyn std::any::Any>{Some(self)}
 fn as_any_mut(&mut self)->Option<&mut dyn std::any::Any>{Some(self)}
}
