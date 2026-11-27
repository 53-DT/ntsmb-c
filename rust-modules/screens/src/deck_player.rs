//! DeckFiles player surface. This is a backend-neutral view/controller boundary: it speaks only
//! playback-contract identities and UI primitives. The app composition executes actions through
//! the URI/feed drivers and the existing C/Starfish player stack; no Plex route or media structs
//! enter this screen.
use plx_playback_contract::{ContentTime, TrackChoice, TrackKind};
use plx_ui::{Painter, Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum PlayerPanel { #[default] Hidden, Audio, Subtitles, More, Timing, Recovery }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum PlayerKey { Up,Down,Left,Right,Ok,Back,PlayPause,SeekBack,SeekForward }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum RecoveryStep { ReopenSource, RebindVideoPlane, RemuxContainer, RetryUri, RetryFeed, RestartPlayback }
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct PlayerTrack { pub id:u64,pub label:String,pub language:String,pub codec:String,pub selected:bool }
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct PlayerSnapshot {
    pub title:String,pub detail:String,pub position_ms:u64,pub duration_ms:Option<u64>,
    pub paused:bool,pub buffered_percent:u8,pub audio_tracks:Vec<PlayerTrack>,
    pub subtitle_tracks:Vec<PlayerTrack>,pub next_title:Option<String>,pub error:Option<String>,
}
impl Default for PlayerSnapshot { fn default()->Self {Self{title:String::new(),detail:String::new(),position_ms:0,duration_ms:None,paused:true,buffered_percent:0,audio_tracks:Vec::new(),subtitle_tracks:Vec::new(),next_title:None,error:None}}}

#[derive(Clone,Debug,PartialEq,Eq)]
pub enum PlayerAction {
    Play,Pause,Seek(ContentTime),SelectTrack{kind:TrackKind,choice:TrackChoice},
    AdjustSubtitleOffset(i32),ResetSubtitleOffset,Next,Stop,
    Recover(RecoveryStep),OpenPanel(PlayerPanel),ClosePanel,
}
pub struct DeckPlayerScreen {
    pub snapshot:PlayerSnapshot,pub panel:PlayerPanel,pub focused:usize,pub hud_visible:bool,
    pub subtitle_offset_ms:i32,pub recovery_steps:Vec<RecoveryStep>,pub last_action:Option<String>,
}
impl Default for DeckPlayerScreen { fn default()->Self {Self::new(PlayerSnapshot::default())} }
impl DeckPlayerScreen {
    pub fn new(snapshot:PlayerSnapshot)->Self {Self{snapshot,panel:PlayerPanel::Hidden,focused:0,hud_visible:true,subtitle_offset_ms:0,
        recovery_steps:vec![RecoveryStep::ReopenSource,RecoveryStep::RebindVideoPlane,RecoveryStep::RemuxContainer,RecoveryStep::RetryUri,RecoveryStep::RetryFeed,RecoveryStep::RestartPlayback],last_action:None}}
    pub fn update_snapshot(&mut self,snapshot:PlayerSnapshot){self.snapshot=snapshot;self.focused=self.focused.min(self.rows().len().saturating_sub(1));}
    pub fn open(&mut self,panel:PlayerPanel)->PlayerAction{self.panel=panel;self.focused=0;self.hud_visible=true;PlayerAction::OpenPanel(panel)}
    pub fn close(&mut self)->PlayerAction{self.panel=PlayerPanel::Hidden;self.focused=0;PlayerAction::ClosePanel}
    pub fn rows(&self)->Vec<String>{match self.panel{
        PlayerPanel::Audio=>self.snapshot.audio_tracks.iter().map(|t|format!("{}  ·  {}  ·  {}",t.label,t.language,t.codec)).collect(),
        PlayerPanel::Subtitles=>{let mut rows=vec!["Off".to_string()];rows.extend(self.snapshot.subtitle_tracks.iter().map(|t|format!("{}  ·  {}  ·  {}",t.label,t.language,t.codec)));rows},
        PlayerPanel::More=>vec!["Subtitle timing".into(),"Recovery".into(),"Play next".into(),"Close player".into()],
        PlayerPanel::Timing=>vec![format!("Offset  {:+} ms",self.subtitle_offset_ms),"− 250 ms".into,"+ 250 ms".into,"Reset timing".into()],
        PlayerPanel::Recovery=>self.recovery_steps.iter().map(|s|recovery_label(*s).to_string()).collect(),
        PlayerPanel::Hidden=>Vec::new()}}
    pub fn handle_key(&mut self,key:PlayerKey)->Option<PlayerAction>{
        if self.panel==PlayerPanel::Hidden {
            return match key {
                PlayerKey::Ok|PlayerKey::PlayPause=>Some(if self.snapshot.paused{PlayerAction::Play}else{PlayerAction::Pause}),
                PlayerKey::Left|PlayerKey::SeekBack=>Some(self.seek_relative(-10_000)),
                PlayerKey::Right|PlayerKey::SeekForward=>Some(self.seek_relative(10_000)),
                PlayerKey::Up=>{self.hud_visible=true;Some(self.open(PlayerPanel::More))},
                PlayerKey::Down=>{self.hud_visible=!self.hud_visible;None},
                PlayerKey::Back=>Some(PlayerAction::Stop),
            };
        }
        let count=self.rows().len();
        match key {
            PlayerKey::Up=>{self.focused=self.focused.saturating_sub(1);None},
            PlayerKey::Down=>{if count>0{self.focused=(self.focused+1).min(count-1);}None},
            PlayerKey::Back=>Some(self.close()),
            PlayerKey::Left|PlayerKey::Right if self.panel==PlayerPanel::Timing=>{
                let delta=if key==PlayerKey::Left{-250}else{250};
                self.subtitle_offset_ms=self.subtitle_offset_ms.saturating_add(delta);
                Some(PlayerAction::AdjustSubtitleOffset(delta))
            },
            PlayerKey::Ok=>self.activate_focused(),
            PlayerKey::Left|PlayerKey::Right|PlayerKey::SeekBack|PlayerKey::SeekForward|PlayerKey::PlayPause=>None,
        }
    }
    fn activate_focused(&mut self)->Option<PlayerAction>{
        match self.panel {
            PlayerPanel::Audio=>{let t=self.snapshot.audio_tracks.get(self.focused)?;Some(PlayerAction::SelectTrack{kind:TrackKind::Audio,choice:TrackChoice::Selected(t.id)})},
            PlayerPanel::Subtitles=>{if self.focused==0{Some(PlayerAction::SelectTrack{kind:TrackKind::Subtitle,choice:TrackChoice::Off})}else{let t=self.snapshot.subtitle_tracks.get(self.focused-1)?;Some(PlayerAction::SelectTrack{kind:TrackKind::Subtitle,choice:TrackChoice::Selected(t.id)})}},
            PlayerPanel::More=>match self.focused{0=>Some(self.open(PlayerPanel::Timing)),1=>Some(self.open(PlayerPanel::Recovery)),2=>Some(PlayerAction::Next),3=>Some(PlayerAction::Stop),_=>None},
            PlayerPanel::Timing=>if self.focused==3{self.subtitle_offset_ms=0;Some(PlayerAction::ResetSubtitleOffset)}else{None},
            PlayerPanel::Recovery=>self.recovery_steps.get(self.focused).copied().map(PlayerAction::Recover),
            PlayerPanel::Hidden=>None,
        }
    }
    fn seek_relative(&self,delta:i64)->PlayerAction{
        let base=self.snapshot.position_ms.min(i64::MAX as u64) as i64;
        let mut target=base.saturating_add(delta).max(0) as u64;
        if let Some(duration)=self.snapshot.duration_ms{target=target.min(duration);}
        PlayerAction::Seek(ContentTime::from_millis(target))
    }
    pub fn playlist_next(&self)->Option<PlayerAction>{self.snapshot.next_title.as_ref().map(|_|PlayerAction::Next)}
    pub fn set_subtitle_offset(&mut self,offset_ms:i32){self.subtitle_offset_ms=offset_ms;}
    pub fn draw(&self,p:Painter,r:Rect){
        if self.hud_visible||self.panel!=PlayerPanel::Hidden {
            let bar=Rect::new(r.x+48.0,r.y+r.h-214.0,r.w-96.0,166.0);
            p.rrect(bar,16.0,16.0,theme::DECKFILES_SIDEBAR);
            TextView::new(&self.snapshot.title,theme::size::HEADLINE,theme::TEXT_PRIMARY).draw(p,Rect::new(bar.x+28.0,bar.y+16.0,bar.w-56.0,42.0));
            TextView::new(&self.snapshot.detail,theme::size::CAPTION,theme::TEXT_SECONDARY).draw(p,Rect::new(bar.x+28.0,bar.y+58.0,bar.w-56.0,30.0));
            let pos=format_time(self.snapshot.position_ms);let dur=self.snapshot.duration_ms.map(format_time).unwrap_or_else(||"--:--".into());
            TextView::new(&format!("{pos}  /  {dur}"),theme::size::CAPTION,theme::TEXT_PRIMARY).draw(p,Rect::new(bar.x+28.0,bar.y+96.0,160.0,28.0));
            let track=Rect::new(bar.x+208.0,bar.y+107.0,bar.w-236.0,6.0);
            p.rrect(track,3.0,3.0,theme::CONTROL_IDLE_FILL);
            if let Some(duration)=self.snapshot.duration_ms.filter(|d|*d>0){
                let ratio=(self.snapshot.position_ms.min(duration) as f32/duration as f32).clamp(0.0,1.0);
                p.rrect(Rect::new(track.x,track.y,track.w*ratio,track.h),3.0,3.0,theme::ACCENT);
            }
            let transport=if self.snapshot.paused{"▶  Play"}else{"Ⅱ  Pause"};
            TextView::new(transport,theme::size::BODY,theme::TEXT_PRIMARY).draw(p,Rect::new(bar.x+28.0,bar.y+128.0,150.0,30.0));
            TextView::new("Audio     Subtitles     More",theme::size::CAPTION,theme::TEXT_SECONDARY).draw(p,Rect::new(bar.x+210.0,bar.y+128.0,bar.w-238.0,30.0));
        }
        if self.panel!=PlayerPanel::Hidden {
            let panel=Rect::new(r.x+r.w-520.0,r.y+80.0,460.0,r.h-160.0);
            p.rrect(panel,16.0,16.0,theme::DECKFILES_SETTINGS);
            let title=match self.panel{PlayerPanel::Audio=>"Audio",PlayerPanel::Subtitles=>"Subtitles",PlayerPanel::More=>"More",PlayerPanel::Timing=>"Subtitle timing",PlayerPanel::Recovery=>"Playback recovery",PlayerPanel::Hidden=>""};
            TextView::new(title,theme::size::TITLE,theme::TEXT_PRIMARY).draw(p,Rect::new(panel.x+28.0,panel.y+22.0,panel.w-56.0,42.0));
            for(i,row)in self.rows().iter().take(10).enumerate(){
                let y=panel.y+88.0+i as f32*54.0;
                if i==self.focused{p.rrect(Rect::new(panel.x+14.0,y,panel.w-28.0,48.0),8.0,8.0,theme::CONTROL_IDLE_FILL);}
                TextView::new(row,theme::size::CAPTION,if i==self.focused{theme::ACCENT_INK}else{theme::TEXT_SECONDARY}).draw(p,Rect::new(panel.x+30.0,y+6.0,panel.w-60.0,36.0));
            }
        }
        if let Some(error)=&self.snapshot.error{
            let banner=Rect::new(r.x+48.0,r.y+48.0,r.w-96.0,58.0);
            p.rrect(banner,10.0,10.0,theme::CONTROL_DANGER_IDLE_FILL);
            TextView::new(error,theme::size::CAPTION,theme::DANGER).draw(p,Rect::new(banner.x+20.0,banner.y+10.0,banner.w-40.0,38.0));
        }
    }
}
fn recovery_label(step:RecoveryStep)->&'static str{match step{RecoveryStep::ReopenSource=>"Reopen source",RecoveryStep::RebindVideoPlane=>"Rebind video plane",RecoveryStep::RemuxContainer=>"Try compatible container",RecoveryStep::RetryUri=>"Retry direct URI",RecoveryStep::RetryFeed=>"Retry packet feed",RecoveryStep::RestartPlayback=>"Restart playback"}}
fn format_time(ms:u64)->String{let seconds=ms/1000;format!("{:02}:{:02}:{:02}",seconds/3600,(seconds/60)%60,seconds%60)}
#[cfg(test)]mod tests{
    use super::*;
    fn fixture()->PlayerSnapshot{PlayerSnapshot{title:"Film".into(),detail:"2026 · 1080p".into(),position_ms:30_000,duration_ms:Some(90_000),paused:true,buffered_percent:60,
        audio_tracks:vec![PlayerTrack{id:1,label:"English".into(),language:"eng".into(),codec:"E-AC-3".into(),selected:true}],
        subtitle_tracks:vec![PlayerTrack{id:2,label:"English".into(),language:"eng".into(),codec:"SRT".into(),selected:false}],next_title:Some("Next episode".into()),error:None}}
    #[test]fn seek_is_clamped_to_media_bounds(){let s=DeckPlayerScreen::new(fixture());assert_eq!(s.seek_relative(-60_000),PlayerAction::Seek(ContentTime::from_millis(0)));assert_eq!(s.seek_relative(90_000),PlayerAction::Seek(ContentTime::from_millis(90_000)));}
    #[test]fn subtitle_picker_has_off_then_tracks(){let mut s=DeckPlayerScreen::new(fixture());s.open(PlayerPanel::Subtitles);assert_eq!(s.rows()[0],"Off");assert_eq!(s.rows().len(),2);assert_eq!(s.handle_key(PlayerKey::Down),None);assert_eq!(s.handle_key(PlayerKey::Ok),Some(PlayerAction::SelectTrack{kind:TrackKind::Subtitle,choice:TrackChoice::Selected(2)}));}
    #[test]fn timing_adjustment_is_incremental_and_resettable(){let mut s=DeckPlayerScreen::new(fixture());s.open(PlayerPanel::Timing);assert_eq!(s.handle_key(PlayerKey::Right),Some(PlayerAction::AdjustSubtitleOffset(250)));assert_eq!(s.subtitle_offset_ms,250);s.focused=3;assert_eq!(s.handle_key(PlayerKey::Ok),Some(PlayerAction::ResetSubtitleOffset));assert_eq!(s.subtitle_offset_ms,0);}
    #[test]fn more_opens_recovery_and_playlist_next(){let mut s=DeckPlayerScreen::new(fixture());s.open(PlayerPanel::More);s.focused=1;assert_eq!(s.handle_key(PlayerKey::Ok),Some(PlayerAction::OpenPanel(PlayerPanel::Recovery)));assert_eq!(s.playlist_next(),Some(PlayerAction::Next));}
}