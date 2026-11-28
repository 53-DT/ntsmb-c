(() => {
 const $=s=>document.querySelector(s), $$=s=>Array.from(document.querySelectorAll(s));
 const backdrop=$('#modal-backdrop'), modal=$('.modal'); let previousFocus=null, guest=false, kind='smb';
 const focusables=()=>$$('button:not(:disabled),input:not(:disabled),[data-focus]').filter(e=>!e.closest('.hidden')&&!e.closest('.backdrop.hidden'));
 function openModal(){previousFocus=document.activeElement;backdrop.classList.remove('hidden');setKind('smb');const first=$('.module-tab');first.focus();}
 function closeModal(){backdrop.classList.add('hidden');if(previousFocus&&previousFocus.focus)previousFocus.focus();}
 function setKind(next){kind=next;$$('.module-tab').forEach(b=>b.classList.toggle('selected',b.dataset.kind===kind));$('#smb-fields').classList.toggle('hidden',kind!=='smb');$('#local-fields').classList.toggle('hidden',kind!=='local');$('#gdrive-fields').classList.toggle('hidden',kind!=='gdrive');}
 $('#add-source').addEventListener('click',openModal);$('#close-modal').addEventListener('click',closeModal);$('#cancel-modal').addEventListener('click',closeModal);
 $$('.module-tab').forEach(b=>b.addEventListener('click',()=>setKind(b.dataset.kind)));
 $('#guest-row').addEventListener('click',()=>{guest=!guest;$('#guest-switch').classList.toggle('on',guest);$('#credentials').classList.toggle('hidden',guest);$('#vault-row').classList.toggle('hidden',guest);});
 $('#source-form').addEventListener('submit',e=>{e.preventDefault();closeModal();});
 $('#select-mode').addEventListener('click',e=>{e.currentTarget.textContent=e.currentTarget.textContent==='Select'?'Done':'Select';});
 $('#grid-view').addEventListener('click',()=>setView('grid'));$('#list-view').addEventListener('click',()=>setView('list'));
 function setView(v){$('#folder-grid').classList.toggle('list',v==='list');$('#grid-view').classList.toggle('chosen',v==='grid');$('#list-view').classList.toggle('chosen',v==='list');}
 $('#search-toggle').addEventListener('click',()=>{$('#search-row').classList.toggle('hidden');if(!$('#search-row').classList.contains('hidden'))$('#search').focus();});
 $('#clear-search').addEventListener('click',()=>{$('#search').value='';filter();});
 $('#search').addEventListener('input',filter);
 function filter(){const q=$('#search').value.toLowerCase();$$('.folder-card').forEach(c=>c.classList.toggle('hidden',!c.textContent.toLowerCase().includes(q)));}
 // Spatial D-pad navigation: choose the nearest visible control in the requested direction.
 function moveFocus(key){const items=focusables().filter(e=>e.getClientRects().length);const current=document.activeElement;if(!items.includes(current)){const first=items[0]||document.body;if(typeof first.focus==='function')first.focus();return;}
 const a=current.getBoundingClientRect(), ax=a.left+a.width/2, ay=a.top+a.height/2;let best=null,bestScore=Infinity;
 for(const el of items){if(el===current)continue;const r=el.getBoundingClientRect(),x=r.left+r.width/2,y=r.top+r.height/2,dx=x-ax,dy=y-ay;let primary,secondary;
 if(key==='ArrowRight'){if(dx<=4)continue;primary=dx;secondary=Math.abs(dy);}
 if(key==='ArrowLeft'){if(dx>=-4)continue;primary=-dx;secondary=Math.abs(dy);}
 if(key==='ArrowDown'){if(dy<=4)continue;primary=dy;secondary=Math.abs(dx);}
 if(key==='ArrowUp'){if(dy>=-4)continue;primary=-dy;secondary=Math.abs(dx);}
 const score=primary+secondary*1.65;if(score<bestScore){bestScore=score;best=el;}
 }if(best)best.focus();}
 document.addEventListener('keydown',e=>{
  if(['ArrowUp','ArrowDown','ArrowLeft','ArrowRight'].includes(e.key)){
   // Let the TV/browser keyboard handle directional caret movement only while editing a text field.
   if(document.activeElement.matches('input,textarea'))return;
   e.preventDefault();moveFocus(e.key);
  } else if(e.key==='Enter'||e.key===' '){if(document.activeElement.tagName==='BUTTON'){e.preventDefault();document.activeElement.click();}}
  else if(e.key==='Escape'||e.key==='Backspace'){if(!backdrop.classList.contains('hidden')){e.preventDefault();closeModal();}else if(!$('#search-row').classList.contains('hidden')){$('#search-row').classList.add('hidden');$('#search').value='';filter();}}
 });
 // Initial TV focus begins on the first folder tile, matching the reference screen.
 window.addEventListener('load',()=>{const first=$('.folder-card');if(first)first.focus();});
})();