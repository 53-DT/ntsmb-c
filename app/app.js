(() => {
 const $=s=>document.querySelector(s), $$=s=>Array.from(document.querySelectorAll(s));
 const backdrop=$('#modal-backdrop'), modal=$('.modal');
 let previousFocus=null,guest=false,kind='smb',currentView='grid';
 const visible=e=>!!(e&&e.getClientRects().length&&!e.closest('.hidden')&&!e.disabled);
 const focusables=()=>{
  const scope=backdrop.classList.contains('hidden')?document:modal;
  return Array.from(scope.querySelectorAll('button,input,[data-focus]')).filter(visible);
 };
 function setFocus(el){if(el&&typeof el.focus==='function'){el.focus();if(el.scrollIntoView)el.scrollIntoView({block:'nearest',inline:'nearest'});}}
 function openModal(){previousFocus=document.activeElement;backdrop.classList.remove('hidden');modal.scrollTop=0;setKind('smb');setFocus($('.module-tab'));}
 function closeModal(){backdrop.classList.add('hidden');setFocus(previousFocus);}
 function setKind(next){kind=next;$$('.module-tab').forEach(b=>b.classList.toggle('selected',b.dataset.kind===kind));$('#smb-fields').classList.toggle('hidden',kind!=='smb');$('#local-fields').classList.toggle('hidden',kind!=='local');$('#gdrive-fields').classList.toggle('hidden',kind!=='gdrive');}
 $('#add-source').addEventListener('click',openModal);$('#close-modal').addEventListener('click',closeModal);$('#cancel-modal').addEventListener('click',closeModal);
 $$('.module-tab').forEach(b=>b.addEventListener('click',()=>setKind(b.dataset.kind)));
 $('#guest-row').addEventListener('click',()=>{guest=!guest;$('#guest-switch').classList.toggle('on',guest);$('#credentials').classList.toggle('hidden',guest);$('#vault-row').classList.toggle('hidden',guest);});
 $('#source-form').addEventListener('submit',e=>{e.preventDefault();closeModal();});
 $('#select-mode').addEventListener('click',e=>{e.currentTarget.textContent=e.currentTarget.textContent==='Select'?'Done':'Select';});
 $('#grid-view').addEventListener('click',()=>setView('grid'));$('#list-view').addEventListener('click',()=>setView('list'));
 function setView(v){currentView=v;$('#folder-grid').classList.toggle('list',v==='list');$('#grid-view').classList.toggle('chosen',v==='grid');$('#list-view').classList.toggle('chosen',v==='list');try{localStorage.setItem('deckfiles-folder-view',v);}catch(_){} }
 const searchBox=$('#search-box'),searchInput=$('#search');
 function openSearch(){searchBox.classList.add('open');setFocus(searchInput);}
 function closeSearch(){searchBox.classList.remove('open','has-value');searchInput.value='';filter();setFocus($('#search-toggle'));}
 $('#search-toggle').addEventListener('click',()=>{if(searchBox.classList.contains('open')){if(searchInput.value){searchInput.value='';filter();searchInput.focus();}else closeSearch();}else openSearch();});
 $('#clear-search').addEventListener('click',()=>{searchInput.value='';filter();setFocus(searchInput);});
 searchInput.addEventListener('input',()=>{searchBox.classList.toggle('has-value',!!searchInput.value);filter();});
 function filter(){const q=searchInput.value.toLowerCase();$('.folder-card').forEach(c=>c.classList.toggle('hidden',!c.textContent.toLowerCase().includes(q)));}
 function moveFocus(key){
  const items=focusables(),current=document.activeElement;
  if(!items.includes(current)){setFocus(items[0]);return;}
  const a=current.getBoundingClientRect(),ax=a.left+a.width/2,ay=a.top+a.height/2;
  let best=null,bestScore=Infinity;
  for(const el of items){
   if(el===current)continue;
   const r=el.getBoundingClientRect(),x=r.left+r.width/2,y=r.top+r.height/2,dx=x-ax,dy=y-ay;
   let primary,secondary;
   if(key==='ArrowRight'){if(dx<=3)continue;primary=dx;secondary=Math.abs(dy);}
   else if(key==='ArrowLeft'){if(dx>=-3)continue;primary=-dx;secondary=Math.abs(dy);}
   else if(key==='ArrowDown'){if(dy<=3)continue;primary=dy;secondary=Math.abs(dx);}
   else {if(dy>=-3)continue;primary=-dy;secondary=Math.abs(dx);}
   // Prefer controls in the same row/column, but still allow diagonal transitions at panel edges.
   const score=primary+secondary*1.35+(secondary>primary?secondary*.35:0);
   if(score<bestScore){bestScore=score;best=el;}
  }
  if(best)setFocus(best);
 }
 function keyName(e){
  if(e.key&&e.key.startsWith('Arrow'))return e.key;
  return ({37:'ArrowLeft',38:'ArrowUp',39:'ArrowRight',40:'ArrowDown'})[e.keyCode]||e.key;
 }
 document.addEventListener('keydown',e=>{
  const key=keyName(e);
  if(['ArrowUp','ArrowDown','ArrowLeft','ArrowRight'].includes(key)){if(document.activeElement===searchInput)return;e.preventDefault();moveFocus(key);}
  else if(key==='Enter'||key===' '||e.keyCode===13){if(document.activeElement.tagName==='BUTTON'){e.preventDefault();document.activeElement.click();}}
  else if(key==='Escape'||key==='Backspace'||e.keyCode===461||e.keyCode===8){
   if(!backdrop.classList.contains('hidden')){e.preventDefault();closeModal();}
   else if(searchBox.classList.contains('open')){e.preventDefault();closeSearch();}
  }
 });
 // Focus styling is driven exclusively by the real focused element; no tile keeps a stale highlight.
 window.addEventListener('load',()=>setFocus($('.folder-card')));
 document.addEventListener('focusin',e=>{ $$('.folder-card').forEach(c=>c.classList.remove('focused')); });
})();