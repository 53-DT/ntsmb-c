//! DeckFiles Files surface. Presentation-only: the application supplies neutral source and entry
//! snapshots and performs all I/O in its worker layer. No provider, vault, app, or sibling imports.
use plx_ui::{Painter, Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowserMode { Grid, #[default] List }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceItem { pub id: String, pub label: String, pub kind_label: String, pub available: bool, pub has_saved_credential: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind { Folder, Video, Subtitle, Other }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileItem { pub name: String, pub path: String, pub kind: FileKind, pub size_bytes: u64 }
impl FileItem { pub fn is_folder(&self) -> bool { self.kind == FileKind::Folder } pub fn is_media(&self) -> bool { self.kind == FileKind::Video } }

pub struct FilesBrowserScreen {
    pub sources: Vec<SourceItem>, pub active_source: Option<String>, pub current_path: String,
    pub entries: Vec<FileItem>, pub selected: usize, pub mode: BrowserMode, pub search: String,
    pub loading: bool, pub error: Option<String>, pub select_mode: bool, history: Vec<String>,
}
impl Default for FilesBrowserScreen { fn default() -> Self { Self::new() } }
impl FilesBrowserScreen {
    pub fn new() -> Self { Self { sources: Vec::new(), active_source: None, current_path: String::new(),
        entries: Vec::new(), selected: 0, mode: BrowserMode::Grid, search: String::new(),
        loading: false, error: None, select_mode: false, history: Vec::new() } }
    pub fn set_sources(&mut self, sources: Vec<SourceItem>) {
        self.sources = sources;
        if self.active_source.as_ref().is_some_and(|id| !self.sources.iter().any(|s| &s.id == id)) {
            self.active_source = None; self.reset_directory();
        }
    }
    pub fn choose_source(&mut self, id: &str) -> bool {
        if !self.sources.iter().any(|s| s.id == id && s.available) { return false; }
        self.active_source = Some(id.to_owned()); self.reset_directory(); true
    }
    pub fn set_entries(&mut self, mut entries: Vec<FileItem>) {
        entries.sort_by(|a,b| b.is_folder().cmp(&a.is_folder()).then_with(|| natural_cmp(&a.name,&b.name)));
        self.entries = entries; self.loading = false;
        self.selected = self.selected.min(self.visible_entries().len().saturating_sub(1)); self.error = None;
    }
    pub fn set_loading(&mut self, loading: bool) { self.loading = loading; if loading { self.error = None; } }
    pub fn set_error(&mut self, error: Option<String>) { self.loading = false; self.error = error; }
    pub fn visible_entries(&self) -> Vec<&FileItem> {
        let q = self.search.trim().to_lowercase();
        self.entries.iter().filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q)).collect()
    }
    pub fn move_selection(&mut self, delta: isize) {
        let n = self.visible_entries().len();
        self.selected = if n == 0 { 0 } else { (self.selected as isize + delta).clamp(0,n as isize-1) as usize };
    }
    pub fn selected_entry(&self) -> Option<&FileItem> { self.visible_entries().get(self.selected).copied() }
    pub fn enter_selected(&mut self) -> Option<String> {
        let entry = self.selected_entry()?.clone(); if !entry.is_folder() { return None; }
        self.history.push(self.current_path.clone()); self.current_path = entry.path;
        self.entries.clear(); self.selected = 0; Some(self.current_path.clone())
    }
    pub fn go_back(&mut self) -> Option<String> {
        self.current_path = self.history.pop()?; self.entries.clear(); self.selected = 0; Some(self.current_path.clone())
    }
    pub fn breadcrumb(&self) -> Vec<&str> {
        let mut parts = vec!["Files"]; parts.extend(self.current_path.split('/').filter(|p| !p.is_empty())); parts
    }
    pub fn playlist_paths(&self, selected_path: &str) -> Vec<String> {
        if !self.entries.iter().any(|e| e.is_media() && e.path == selected_path) { return Vec::new(); }
        self.entries.iter().filter(|e| e.is_media()).map(|e| e.path.clone()).collect()
    }
    pub fn toggle_mode(&mut self) { self.mode = if self.mode == BrowserMode::Grid { BrowserMode::List } else { BrowserMode::Grid }; }
    pub fn set_search(&mut self, value: String) { self.search = value; self.selected = 0; }
    pub fn reset_directory(&mut self) {
        self.current_path.clear(); self.entries.clear(); self.history.clear(); self.selected = 0;
        self.loading = false; self.error = None; self.search.clear();
    }
    pub fn draw(&self, p: Painter, r: Rect) {
        let side = Rect::new(r.x,r.y,300.0,r.h);
        let main = Rect::new(r.x+300.0,r.y,(r.w-300.0).max(0.0),r.h);
        p.rrect(side,0.0,0.0,theme::DECKFILES_SIDEBAR); p.rrect(main,0.0,0.0,theme::DECKFILES_CANVAS);
        TextView::new("Files",theme::size::HEADLINE,theme::TEXT_PRIMARY).draw(p,Rect::new(side.x+28.0,side.y+30.0,240.0,48.0));
        TextView::new("Settings",theme::size::BODY,theme::TEXT_SECONDARY).draw(p,Rect::new(side.x+28.0,side.y+92.0,240.0,42.0));
        TextView::new("SOURCES",theme::size::MICRO,theme::TEXT_TERTIARY).draw(p,Rect::new(side.x+28.0,side.y+166.0,240.0,32.0));
        TextView::new("+ Add source",theme::size::CAPTION,theme::TEXT_PRIMARY).draw(p,Rect::new(side.x+28.0,side.y+206.0,240.0,38.0));
        for (i,s) in self.sources.iter().enumerate() {
            let y=side.y+260.0+i as f32*52.0; let active=self.active_source.as_deref()==Some(s.id.as_str());
            if active { p.rrect(Rect::new(side.x+12.0,y-4.0,276.0,46.0),8.0,8.0,theme::CONTROL_IDLE_FILL); }
            TextView::new(&s.label,theme::size::CAPTION,if active {theme::ACCENT_INK}else{theme::TEXT_SECONDARY}).draw(p,Rect::new(side.x+28.0,y,244.0,36.0));
        }
        let crumb=self.breadcrumb().join("  ›  ");
        TextView::new(&crumb,theme::size::BODY,theme::TEXT_PRIMARY).draw(p,Rect::new(main.x+36.0,main.y+26.0,main.w-72.0,48.0));
        TextView::new("OK open     Left / Right sources     Play add     Pause settings",theme::size::CAPTION,theme::TEXT_SECONDARY).draw(p,Rect::new(main.x+36.0,main.y+88.0,main.w-72.0,38.0));
        if self.loading { TextView::new("Loading…",theme::size::BODY,theme::TEXT_SECONDARY).draw(p,Rect::new(main.x+36.0,main.y+170.0,main.w-72.0,44.0)); }
        else if let Some(error)=&self.error { TextView::new(error,theme::size::CAPTION,theme::DANGER).draw(p,Rect::new(main.x+36.0,main.y+170.0,main.w-72.0,64.0)); }
        else { for (i,e) in self.visible_entries().iter().take(10).enumerate() {
            let h=if self.mode==BrowserMode::List {58.0}else{76.0}; let y=main.y+154.0+i as f32*h; let hot=i==self.selected;
            if hot { p.rrect(Rect::new(main.x+24.0,y,main.w-48.0,h-6.0),8.0,8.0,theme::CONTROL_IDLE_FILL); }
            let prefix=match e.kind {FileKind::Folder=>"▸  ",FileKind::Video=>"▶  ",_=>"   "};
            let label=format!("{prefix}{}",e.name);
            TextView::new(&label,theme::size::BODY,if hot {theme::ACCENT_INK}else{theme::TEXT_SECONDARY}).draw(p,Rect::new(main.x+42.0,y+5.0,main.w-84.0,h-12.0));
        }}
    }
}
fn natural_cmp(left:&str,right:&str)->std::cmp::Ordering {
    let mut a=left.chars().peekable(); let mut b=right.chars().peekable();
    loop { match (a.peek().copied(),b.peek().copied()) {
        (None,None)=>return std::cmp::Ordering::Equal,(None,_)=>return std::cmp::Ordering::Less,(_,None)=>return std::cmp::Ordering::Greater,
        (Some(x),Some(y)) if x.is_ascii_digit()&&y.is_ascii_digit()=>{
            let mut an=String::new();while a.peek().is_some_and(|c|c.is_ascii_digit()){an.push(a.next().unwrap());}
            let mut bn=String::new();while b.peek().is_some_and(|c|c.is_ascii_digit()){bn.push(b.next().unwrap());}
            let ord=an.trim_start_matches('0').len().cmp(&bn.trim_start_matches('0').len()).then_with(||an.trim_start_matches('0').cmp(bn.trim_start_matches('0')));
            if ord!=std::cmp::Ordering::Equal{return ord;}
        }
        _=>{let ord=a.next().unwrap().to_ascii_lowercase().cmp(&b.next().unwrap().to_ascii_lowercase());if ord!=std::cmp::Ordering::Equal{return ord;}}
    }}
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn breadcrumbs_follow_path(){let mut s=FilesBrowserScreen::new();s.current_path="Films/2026".into();assert_eq!(s.breadcrumb(),vec!["Files","Films","2026"]);}
    #[test] fn folders_and_natural_order(){let mut s=FilesBrowserScreen::new();s.set_entries(vec![
        FileItem{name:"Episode 10.mkv".into(),path:"10".into(),kind:FileKind::Video,size_bytes:0},
        FileItem{name:"Episode 2.mkv".into(),path:"2".into(),kind:FileKind::Video,size_bytes:0},
        FileItem{name:"Folder".into(),path:"folder".into(),kind:FileKind::Folder,size_bytes:0}]);
        assert_eq!(s.entries.iter().map(|e|e.name.as_str()).collect::<Vec<_>>(),vec!["Folder","Episode 2.mkv","Episode 10.mkv"]);}
}