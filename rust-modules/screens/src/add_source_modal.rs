//! DeckFiles source form. Pure UI state; the application validates the draft with the provider,
//! runs the connection worker, and persists credentials through the OS vault.
use plx_ui::{Painter,Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum SourceProviderChoice { #[default] Smb, Local, GoogleDrive }
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum ConnectState { #[default] Editing, Connecting, Connected }
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct SourceDraft {
    pub provider:SourceProviderChoice,pub label:String,pub host:String,pub port:u16,pub share:String,
    pub username:String,pub password:String,pub guest:bool,pub save_password:bool,pub local_folder:String,
}
pub struct AddSourceModal { pub draft:SourceDraft,pub state:ConnectState,pub error:Option<String>,pub focused_field:usize }
impl Default for AddSourceModal {fn default()->Self{Self::new()}}
impl AddSourceModal {
    pub fn new()->Self{Self{draft:SourceDraft{provider:SourceProviderChoice::Smb,label:String::new(),host:String::new(),port:445,share:String::new(),username:String::new(),password:String::new(),guest:false,save_password:false,local_folder:String::new()},state:ConnectState::Editing,error:None,focused_field:0}}
    pub fn provider_cards()->[(SourceProviderChoice,&'static str,bool);3]{[(SourceProviderChoice::Smb,"SMB",true),(SourceProviderChoice::Local,"Local Media",true),(SourceProviderChoice::GoogleDrive,"Google Drive",false)]}
    pub fn select_provider(&mut self,provider:SourceProviderChoice)->Result<(),String>{
        if !Self::provider_cards().iter().any(|(p,_,available)|*p==provider&&*available){return Err("Google Drive is not available yet".into());}
        self.draft.provider=provider;self.error=None;self.focused_field=0;Ok(())
    }
    pub fn field_count(&self)->usize{match self.draft.provider{SourceProviderChoice::Smb=>9,SourceProviderChoice::Local=>3,SourceProviderChoice::GoogleDrive=>0}}
    pub fn move_focus(&mut self,delta:isize){let n=self.field_count();if n>0{self.focused_field=(self.focused_field as isize+delta).clamp(0,n as isize-1)as usize;}}
    pub fn validate(&self)->Result<(),String>{match self.draft.provider{
        SourceProviderChoice::Smb=>{if self.draft.host.trim().is_empty(){return Err("Enter the SMB server address".into());}
            if self.draft.port==0{return Err("Enter a valid SMB port".into());}
            if self.draft.share.trim().is_empty(){return Err("Enter the SMB share name".into());}
            if !self.draft.guest&&self.draft.username.trim().is_empty(){return Err("Enter a username or enable Guest".into());}
            if !self.draft.guest&&self.draft.password.is_empty(){return Err("Enter the SMB password".into());}}
        SourceProviderChoice::Local=>if self.draft.local_folder.trim().is_empty(){return Err("Choose a local media folder".into());},
        SourceProviderChoice::GoogleDrive=>return Err("Google Drive is not available yet".into()),}Ok(())}
    pub fn begin_connect(&mut self)->Result<SourceDraft,String>{if let Err(e)=self.validate(){self.error=Some(e.clone());return Err(e);}
        self.state=ConnectState::Connecting;self.error=None;Ok(self.draft.clone())}
    pub fn finish_connect(&mut self,result:Result<(),String>){match result{Ok(())=>{self.state=ConnectState::Connected;self.draft.password.clear();self.error=None;},Err(e)=>{self.state=ConnectState::Editing;self.error=Some(e);}}}
    pub fn cancel(&mut self){self.draft.password.clear();self.state=ConnectState::Editing;self.error=None;}
    pub fn draw(&self,p:Painter,r:Rect){
        p.rrect(r,18.0,18.0,theme::DECKFILES_CANVAS);
        TextView::new("Add source",theme::size::HEADLINE,theme::TEXT_PRIMARY).draw(p,Rect::new(r.x+42.0,r.y+28.0,r.w-84.0,52.0));
        TextView::new("Credentials are sealed on this device",theme::size::CAPTION,theme::TEXT_SECONDARY).draw(p,Rect::new(r.x+42.0,r.y+84.0,r.w-84.0,38.0));
        for(i,(kind,title,available))in Self::provider_cards().iter().enumerate(){let x=r.x+42.0+i as f32*230.0;let active=*kind==self.draft.provider;
            p.rrect(Rect::new(x,r.y+146.0,210.0,72.0),10.0,10.0,if active{theme::CONTROL_IDLE_FILL}else{theme::SURFACE_PANEL});
            TextView::new(title,theme::size::BODY,if active{theme::ACCENT_INK}else{theme::TEXT_SECONDARY}).draw(p,Rect::new(x+16.0,r.y+164.0,178.0,36.0));
            if !available{TextView::new("Coming soon",theme::size::MICRO,theme::TEXT_TERTIARY).draw(p,Rect::new(x+16.0,r.y+218.0,178.0,30.0));}}
        let fields=match self.draft.provider{
            SourceProviderChoice::Smb=>vec![format!("Name: {}",self.draft.label),format!("Server: {}",self.draft.host),format!("Port: {}",self.draft.port),format!("Share: {}",self.draft.share),format!("Username: {}",if self.draft.guest{"(Guest access)"}else{&self.draft.username}),format!("Password: {}",if self.draft.guest{"Disabled for guest".to_string()}else{"•".repeat(self.draft.password.chars().count())}),format!("Guest access: {}",if self.draft.guest{"[✓] On"}else{"[ ] Off"}),format!("Save password: {}",if self.draft.save_password&&!self.draft.guest{"[✓] On"}else{"[ ] Off"}),"[ CONNECT ]".to_string()],
            SourceProviderChoice::Local=>vec![format!("Name: {}",self.draft.label),format!("Folder: {}",self.draft.local_folder),"[ ADD FOLDER ]".to_string()],SourceProviderChoice::GoogleDrive=>Vec::new()};
        for(i,field)in fields.iter().enumerate(){
            let row=Rect::new(r.x+34.0,r.y+252.0+i as f32*44.0,r.w-68.0,40.0);
            if i==self.focused_field { p.rrect(row,7.0,7.0,if i==8{theme::ACCENT}else{theme::CONTROL_IDLE_FILL}); }
            let color=if i==self.focused_field {if i==8{theme::ACCENT_INK}else{theme::ACCENT_INK}}else{theme::TEXT_SECONDARY};
            TextView::new(field,theme::size::CAPTION,color).draw(p,Rect::new(row.x+12.0,row.y+3.0,row.w-24.0,34.0));
        }
        if let Some(error)=&self.error{p.rrect(Rect::new(r.x+36.0,r.y+r.h-132.0,r.w-72.0,46.0),8.0,8.0,theme::CONTROL_DANGER_IDLE_FILL);
            TextView::new(error,theme::size::CAPTION,theme::DANGER).draw(p,Rect::new(r.x+52.0,r.y+r.h-126.0,r.w-104.0,34.0));}
        let footer=match self.state{ConnectState::Connecting=>"Connecting to SMB server…",ConnectState::Connected=>"Connected",ConnectState::Editing=>"↑ ↓ Select field     OK Toggle / Connect     BACK Cancel"};
        TextView::new(footer,theme::size::CAPTION,theme::TEXT_PRIMARY).draw(p,Rect::new(r.x+42.0,r.y+r.h-66.0,r.w-84.0,42.0));
    }
}
#[cfg(test)]mod tests{use super::*;#[test]fn cloud_provider_is_disabled(){assert_eq!(AddSourceModal::provider_cards()[2],(SourceProviderChoice::GoogleDrive,"Google Drive",false));}
#[test]fn guest_smb_does_not_require_a_password(){let mut m=AddSourceModal::new();m.draft.host="nas".into();m.draft.share="media".into();m.draft.username="viewer".into();assert!(m.validate().is_err());m.draft.guest=true;assert!(m.validate().is_ok());}}