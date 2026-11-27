//! DeckFiles settings view model. The application adapts this draft to the OS vault.
//! Exactly four roots are exposed; no Plex, Jellyfin, libraries, accounts, telemetry or system pages.
use plx_ui::{Painter,Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum SettingsRoot{#[default]Playback,AudioSubtitles,Credentials,Recovery}
impl SettingsRoot{
    pub const ALL:[Self;4]=[Self::Playback,Self::AudioSubtitles,Self::Credentials,Self::Recovery];
    pub fn title(self)->&'static str{match self{Self::Playback=>"Playback",Self::AudioSubtitles=>"Audio & Subtitles",Self::Credentials=>"Vault & Credentials",Self::Recovery=>"Recovery"}}
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct DeckSettingsDraft{
    pub skip_interval_seconds:u32,pub instant_dpad_seeking:bool,pub hdr_overlay_brightness_percent:u8,
    pub show_remaining_time:bool,pub hud_auto_hide_seconds:u8,pub compact_hud:bool,pub continuously_play_next:bool,
    pub preferred_audio_language:String,pub subtitles_mode:String,pub subtitle_language:String,pub forced_subtitles:String,
    pub subtitle_size:String,pub subtitle_position:String,pub subtitle_color:String,
    pub auto_recover_video_errors:bool,pub prefer_remux_containers:bool,pub sandbox_video_plane_rebind:bool,
}
impl Default for DeckSettingsDraft{fn default()->Self{Self{skip_interval_seconds:10,instant_dpad_seeking:true,hdr_overlay_brightness_percent:100,show_remaining_time:true,hud_auto_hide_seconds:5,compact_hud:false,continuously_play_next:true,preferred_audio_language:"original".into(),subtitles_mode:"Always".into(),subtitle_language:"eng".into(),forced_subtitles:"PreferNonForced".into(),subtitle_size:"Large".into(),subtitle_position:"Low".into(),subtitle_color:"White".into(),auto_recover_video_errors:true,prefer_remux_containers:true,sandbox_video_plane_rebind:true}}}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct CredentialItem{pub source_id:String,pub label:String,pub has_saved_credential:bool,pub guest:bool}
pub struct DeckSettingsScreen{pub settings:DeckSettingsDraft,pub selected_root:SettingsRoot,pub selected_row:usize,pub credentials:Vec<CredentialItem>,pub status:Option<String>}
impl Default for DeckSettingsScreen{fn default()->Self{Self::new(DeckSettingsDraft::default(),Vec::new())}}
impl DeckSettingsScreen{
    pub fn new(settings:DeckSettingsDraft,credentials:Vec<CredentialItem>)->Self{Self{settings,selected_root:SettingsRoot::Playback,selected_row:0,credentials,status:None}}
    pub fn select_root(&mut self,root:SettingsRoot){self.selected_root=root;self.selected_row=0;self.status=None;}
    pub fn move_row(&mut self,delta:isize){let n=self.rows().len();self.selected_row=if n==0{0}else{(self.selected_row as isize+delta).clamp(0,n as isize-1)as usize};}
    pub fn rows(&self)->Vec<(String,String)>{match self.selected_root{
        SettingsRoot::Playback=>vec![("Skip interval".into(),format!("{} seconds",self.settings.skip_interval_seconds)),("Instant D-pad seeking".into(),on_off(self.settings.instant_dpad_seeking)),("HDR overlay brightness".into(),format!("{}%",self.settings.hdr_overlay_brightness_percent)),("Show remaining time".into(),on_off(self.settings.show_remaining_time)),("HUD auto-hide".into(),format!("{} seconds",self.settings.hud_auto_hide_seconds)),("Compact HUD".into(),on_off(self.settings.compact_hud)),("Continuously play next".into(),on_off(self.settings.continuously_play_next))],
        SettingsRoot::AudioSubtitles=>vec![("Preferred audio language".into(),self.settings.preferred_audio_language.clone()),("Subtitles".into(),self.settings.subtitles_mode.clone()),("Subtitle language".into(),self.settings.subtitle_language.clone()),("Forced subtitles".into(),self.settings.forced_subtitles.clone()),("Subtitle size".into(),self.settings.subtitle_size.clone()),("Subtitle position".into(),self.settings.subtitle_position.clone()),("Subtitle color".into(),self.settings.subtitle_color.clone())],
        SettingsRoot::Credentials=>self.credentials.iter().map(|c|(c.label.clone(),if c.guest{"Guest access".into()}else if c.has_saved_credential{"Credential sealed".into()}else{"Credential not saved".into()})).collect(),
        SettingsRoot::Recovery=>vec![("Auto-recover video errors".into(),on_off(self.settings.auto_recover_video_errors)),("Prefer remux containers".into(),on_off(self.settings.prefer_remux_containers)),("Sandbox video-plane rebind".into(),on_off(self.settings.sandbox_video_plane_rebind))]}}
    pub fn set_status(&mut self,status:Option<String>){self.status=status;}
    pub fn draw(&self,p:Painter,r:Rect){p.rrect(r,0.0,0.0,theme::DECKFILES_SETTINGS);let lw=390.0;
        TextView::new("Settings",theme::size::HEADLINE,theme::TEXT_PRIMARY).draw(p,Rect::new(r.x+36.0,r.y+28.0,lw-60.0,52.0));
        for(i,root)in SettingsRoot::ALL.iter().enumerate(){let y=r.y+112.0+i as f32*76.0;let hot=*root==self.selected_root;
            if hot{p.rrect(Rect::new(r.x+20.0,y,lw-40.0,62.0),10.0,10.0,theme::CONTROL_IDLE_FILL);}
            TextView::new(root.title(),theme::size::BODY,if hot{theme::ACCENT_INK}else{theme::TEXT_SECONDARY}).draw(p,Rect::new(r.x+40.0,y+10.0,lw-80.0,42.0));}
        let x=r.x+lw+24.0;let w=(r.w-lw-60.0).max(0.0);
        TextView::new(self.selected_root.title(),theme::size::TITLE,theme::TEXT_PRIMARY).draw(p,Rect::new(x,r.y+34.0,w,52.0));
        for(i,(label,value))in self.rows().iter().take(10).enumerate(){let y=r.y+116.0+i as f32*64.0;
            if i==self.selected_row{p.rrect(Rect::new(x-12.0,y,w,56.0),8.0,8.0,theme::CONTROL_IDLE_FILL);}
            TextView::new(label,theme::size::CAPTION,if i==self.selected_row{theme::ACCENT_INK}else{theme::TEXT_SECONDARY}).draw(p,Rect::new(x+8.0,y+8.0,w*0.58,40.0));
            TextView::new(value,theme::size::CAPTION,if i==self.selected_row{theme::ACCENT_INK}else{theme::TEXT_TERTIARY}).draw(p,Rect::new(x+w*0.60,y+8.0,w*0.36,40.0));}
        if let Some(status)=&self.status{TextView::new(status,theme::size::CAPTION,theme::TEXT_SECONDARY).draw(p,Rect::new(x,r.y+r.h-56.0,w,36.0));}}
}
fn on_off(v:bool)->String{if v{"On".into()}else{"Off".into()}}
#[cfg(test)]mod tests{use super::*;#[test]fn only_four_roots(){assert_eq!(SettingsRoot::ALL.map(SettingsRoot::title),["Playback","Audio & Subtitles","Vault & Credentials","Recovery"]);}#[test]fn defaults_match_vault_contract(){let s=DeckSettingsDraft::default();assert_eq!(s.skip_interval_seconds,10);assert!(s.instant_dpad_seeking&&s.auto_recover_video_errors);}}