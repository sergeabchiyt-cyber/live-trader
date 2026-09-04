"""Self-contained dashboard (inline CSS/JS, no external deps)."""
DASHBOARD_HTML = r"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>FRVP PoC Trader — Live Dashboard</title>
<style>
:root{--bg:#0b101a;--panel:#121a2b;--panel2:#0d1524;--line:#243049;--tx:#e9edf5;--mut:#8ea0bd;
--gold:#e6b647;--green:#3ddc97;--red:#f06a6a;--blue:#57a7f0;--orange:#f0a04a;--purple:#b187f0}
*{box-sizing:border-box}html,body{margin:0}
body{background:var(--bg);color:var(--tx);font:14px/1.45 'Segoe UI',system-ui,-apple-system,sans-serif}
.wrap{max-width:1480px;margin:0 auto;padding:16px 18px 60px}
header{display:flex;align-items:center;gap:16px;flex-wrap:wrap;border-bottom:1px solid var(--line);padding-bottom:10px;margin-bottom:14px}
h1{font-size:19px;margin:0}.sub{color:var(--mut);font-size:12px}
.pill{background:#1a2437;border:1px solid var(--line);border-radius:20px;padding:3px 11px;font-size:12px}
.pill b{color:var(--gold)}
.dot{width:9px;height:9px;border-radius:50%;display:inline-block;margin-right:5px;vertical-align:baseline}
.g{background:var(--green)}.r{background:var(--red)}.y{background:var(--gold)}.b{background:var(--blue)}
.kpis{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:10px;margin-bottom:14px}
.kpi{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:10px 12px}
.kpi .l{color:var(--mut);font-size:10.5px;text-transform:uppercase;letter-spacing:.4px}
.kpi .v{font-size:20px;font-weight:700;margin-top:2px}
.grid{display:grid;grid-template-columns:minmax(0,2.1fr) minmax(320px,1fr);gap:12px}
@media(max-width:1050px){.grid{grid-template-columns:1fr}}
.card{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:12px 14px;min-width:0}
.card h3{margin:0 0 8px;font-size:12px;color:#cfe0ff;text-transform:uppercase;letter-spacing:.5px}
#chart{width:100%;height:430px;display:block}
svg text{font-family:'Segoe UI',system-ui,sans-serif}
table{width:100%;border-collapse:collapse;font-size:12px}
th{color:var(--mut);text-align:left;font-weight:600;border-bottom:1px solid var(--line);padding:4px 6px;white-space:nowrap}
td{padding:4px 6px;border-bottom:1px solid #1a2336;text-align:left;white-space:nowrap}
.win{color:var(--green)}.lose{color:var(--red)}.gold{color:var(--gold)}
.row{display:flex;gap:10px;margin-top:12px;flex-wrap:wrap}
.row .card{flex:1 1 340px}
.ev{font-size:11.5px;color:var(--mut);padding:2px 0;border-bottom:1px dashed #1a2336;font-family:ui-monospace,Consolas,monospace}
.ev b{color:var(--tx)}
.mono{font-family:ui-monospace,Consolas,monospace;font-size:11.5px}
.footer{color:var(--mut);font-size:11px;margin-top:18px}
#statusline{font-size:12px;color:var(--mut)}
.bigprice{font-size:26px;font-weight:800}
</style></head><body><div class="wrap">
<header>
  <div><h1><span class="dot y"></span>FRVP PoC Live Trader</h1>
  <div class="sub" id="sub">connecting…</div></div>
  <div class="pill">mode <b id="mode">–</b></div>
  <div class="pill">RR <b id="rr">–</b></div>
  <div class="pill">skip 00–01 <b id="skip">–</b></div>
  <div class="pill">trail <b id="trail">–</b></div>
  <div class="pill" id="phasepill">status –</div>
</header>

<div class="kpis">
  <div class="kpi"><div class="l">Last price</div><div class="v gold" id="k_price">–</div></div>
  <div class="kpi"><div class="l">PD PoC</div><div class="v" id="k_poc">–</div></div>
  <div class="kpi"><div class="l">Session open</div><div class="v" id="k_open">–</div></div>
  <div class="kpi"><div class="l">Bias (vs PoC)</div><div class="v" id="k_bias">–</div></div>
  <div class="kpi"><div class="l">Phase</div><div class="v" id="k_phase">–</div></div>
  <div class="kpi"><div class="l">ATR14</div><div class="v" id="k_atr">–</div></div>
  <div class="kpi"><div class="l">Session</div><div class="v" id="k_session">–</div></div>
  <div class="kpi"><div class="l">Equity (paper)</div><div class="v" id="k_eq">–</div></div>
</div>

<div class="grid">
  <div class="card">
    <h3>Price · PD VP / PoC · position &amp; trail levels</h3>
    <svg id="chart"></svg>
    <div id="chartlegend" style="font-size:11px;color:var(--mut)"></div>
  </div>
  <div style="display:flex;flex-direction:column;gap:12px">
    <div class="card">
      <h3>Position / next setup</h3>
      <div id="pos" class="mono" style="font-size:12.5px">–</div>
    </div>
    <div class="card">
      <h3>Prior-session FRVP (48-bin tick-volume profile)</h3>
      <div id="proffile" class="mono" style="font-size:11.5px">–</div>
      <svg id="profile"></svg>
    </div>
  </div>
</div>

<div class="row">
  <div class="card"><h3>Trade log</h3><div style="max-height:220px;overflow:auto"><table id="trades">
    <thead><tr><th>#</th><th>time(UTC)</th><th>session</th><th>side</th><th>entry</th><th>exit</th><th>type</th><th>RR</th><th>pct</th><th>R</th><th>bars</th></tr></thead>
    <tbody></tbody></table></div></div>
  <div class="card"><h3>Equity (paper)</h3><svg id="eqchart" style="width:100%;height:150px"></svg></div>
</div>
<div class="row">
  <div class="card"><h3>Live events</h3><div id="events" style="max-height:200px;overflow:auto"></div></div>
  <div class="card"><h3>Broker / orders</h3><div id="broker" style="max-height:200px;overflow:auto" class="mono"></div></div>
  <div class="card"><h3>Config</h3><div id="cfg" class="mono" style="font-size:11.5px"></div></div>
</div>
<div class="footer" id="statusline">—</div>
</div>

<script>
"use strict";
const $=id=>document.getElementById(id);
const S={bars:[],prof:null,pos:null,eq:[]};
let tradeCount=0;

function fmt(v,d=2){return v==null?'–':Number(v).toLocaleString('en-US',{minimumFractionDigits:d,maximumFractionDigits:d});}
function tsUTC(ts){return ts?new Date(ts*1000).toISOString().slice(11,16)+'Z':'';}

function drawChart(){
  const bars=S.bars; const svg=$('chart');
  const W=svg.clientWidth||900, H=svg.clientHeight||430;
  if(bars.length<3){svg.innerHTML='<text x="20" y="30" fill="#8ea0bd">waiting for bars…</text>';return;}
  const padR=110,padL=8,padT=18,padB=26;
  const cw=W-padR-padL, ch=H-padT-padB;
  let lo=Infinity,hi=-Infinity;
  bars.forEach(b=>{lo=Math.min(lo,b.l);hi=Math.max(hi,b.h);});
  // include profile range & position levels
  let pRange=null;
  if(S.prof&&S.prof.hi>0){pRange={lo:S.prof.lo,hi:S.prof.hi,poc:S.prof.poc,prof:S.prof.prof};}
  if(pRange){lo=Math.min(lo,pRange.lo);hi=Math.max(hi,pRange.hi);}
  const pos=S.pos;
  if(pos){lo=Math.min(lo,pos.entry,pos.sl,pos.tp);hi=Math.max(hi,pos.entry,pos.sl,pos.tp);}
  const pad=(hi-lo)*0.08||1; lo-=pad; hi+=pad;
  const X=i=>padL+i*cw/(bars.length-1||1);
  const Y=p=>padT+ch*(1-(p-lo)/(hi-lo));
  const step=cw/(bars.length-1||1);
  let s='';
  // gridlines + y labels
  const nY=6;
  for(let i=0;i<=nY;i++){const p=lo+(hi-lo)*i/nY;const y=Y(p);
    s+=`<line x1="${padL}" y1="${y}" x2="${W-padR}" y2="${y}" stroke="#1f2a3f" stroke-width="1"/>`+
       `<text x="${W-padR+6}" y="${y+4}" fill="#93a0b8" font-size="10">${fmt(p)}</text>`;}
  // x labels: every ~40 bars by date
  const xl=Math.max(1,Math.ceil(bars.length/8));
  for(let i=0;i<bars.length;i+=xl){const b=bars[i];
    s+=`<text x="${X(i)}" y="${H-8}" fill="#93a0b8" font-size="9.5" text-anchor="middle">${new Date(b.ts*1000).toISOString().slice(5,16)}</text>`;}
  // PD profile histogram (right side, anchored to PD session hi/lo)
  if(pRange&&pRange.prof){
    const mx=Math.max(...pRange.prof)||1;
    const pw=cw*0.13, px0=W-padR-pw;
    pRange.prof.forEach((v,k)=>{if(v<=0)return;
      const fr=k/48, y0=Y(pRange.lo+(pRange.hi-pRange.lo)*fr);
      const y1=Y(pRange.lo+(pRange.hi-pRange.lo)*(fr+1/48));
      s+=`<rect x="${px0}" y="${y1}" width="${pw*v/mx}" height="${Math.max(1,y0-y1)}" fill="#e6b647" opacity="0.30"/>`;});
    s+=`<text x="${px0+4}" y="${padT+8}" fill="#e6b647" font-size="9">PD volume profile</text>`;
    const yp=Y(pRange.poc);
    s+=`<line x1="${padL}" y1="${yp}" x2="${W-padR}" y2="${yp}" stroke="#e6b647" stroke-width="1.3" stroke-dasharray="7 4" opacity="0.9"/>`;
    s+=`<text x="${padL+2}" y="${yp-3}" fill="#e6b647" font-size="9.5">PD PoC ${fmt(pRange.poc,1)}</text>`;
  }
  // candles
  bars.forEach((b,i)=>{const up=b.c>=b.o, col=up?'#3ddc97':'#f06a6a';
    const x=X(i), w=Math.max(1.5,step*0.62);
    s+=`<line x1="${x}" y1="${Y(b.h)}" x2="${x}" y2="${Y(b.l)}" stroke="${col}" stroke-width="1"/>`;
    const yO=Y(b.o),yC=Y(b.c),top=Math.min(yO,yC),hgt=Math.max(1,Math.abs(yO-yC));
    s+=`<rect x="${x-w/2}" y="${top}" width="${w}" height="${hgt}" fill="${col}" opacity="0.92"/>`;});
  // vertical marker: session start (00:00 of today's session?) find where day changes hour==22 boundary ~ skip; add bar where session rolled
  // position levels & markers
  if(pos){
    const col=pos.side==='long'?'#3ddc97':'#f06a6a';
    s+=`<line x1="${padL}" y1="${Y(pos.sl)}" x2="${W-padR}" y2="${Y(pos.sl)}" stroke="${pos.side==='long'?'#f06a6a':'#3ddc97'}" stroke-width="1.2" stroke-dasharray="5 4" opacity="0.8"/>`;
    s+=`<text x="${W-padR-6}" y="${Y(pos.sl)-3}" fill="#93a0b8" font-size="9.5" text-anchor="end">SL ${fmt(pos.sl,1)}</text>`;
    s+=`<line x1="${padL}" y1="${Y(pos.tp)}" x2="${W-padR}" y2="${Y(pos.tp)}" stroke="${pos.side==='long'?'#3ddc97':'#f06a6a'}" stroke-width="1.2" stroke-dasharray="5 4" opacity="0.8"/>`;
    s+=`<text x="${W-padR-6}" y="${Y(pos.tp)-3}" fill="#93a0b8" font-size="9.5" text-anchor="end">TP ${fmt(pos.tp,1)}</text>`;
    s+=`<line x1="${padL}" y1="${Y(pos.entry)}" x2="${W-padR}" y2="${Y(pos.entry)}" stroke="#57a7f0" stroke-width="1.1" opacity="0.85"/>`;
    s+=`<text x="${padL+2}" y="${Y(pos.entry)-4}" fill="#57a7f0" font-size="9.5">${pos.side.toUpperCase()} entry ${fmt(pos.entry,1)} · trail ${fmt(pos.trail_a,2)}R</text>`;
  }
  svg.innerHTML=s;
}

function drawProfile(){
  const p=S.prof; const el=$('proffile');
  if(!p){el.textContent='no prior session yet (bootstrap)';return;}
  const mx=Math.max(...p.prof)||1;
  el.innerHTML=`session range ${fmt(p.lo,1)}–${fmt(p.hi,1)} · <span class="gold">PoC ${fmt(p.poc,2)}</span> · max ${mx.toExponential(2)}`;
  const svg=$('profile');const W=svg.clientWidth||460,H=90;
  let s='';
  const n=p.prof.length;
  const bw=W/(n*2), gap=W/n;
  p.prof.forEach((v,k)=>{const h=(v/mx)*(H-16);const x=k*gap;
    s+=`<rect x="${x}" y="${H-8-h}" width="${Math.max(1,gap*0.7)}" height="${h}" fill="#e6b647" opacity="0.55"/>`;});
  const pocx=(p.poc-p.lo)/(p.hi-p.lo||1);
  s+=`<line x1="${pocx*W}" y1="0" x2="${pocx*W}" y2="${H-8}" stroke="#fff" stroke-width="1.2" stroke-dasharray="3 3"/>`;
  svg.innerHTML=s;
}

function drawEquity(){
  const svg=$('eqchart');const W=svg.clientWidth||640,H=150;
  const d=S.eq;if(!d||d.length<2){svg.innerHTML='<text x="10" y="20" fill="#8ea0bd" font-size="11">waiting…</text>';return;}
  const lo=Math.min(...d.map(p=>p.eq)),hi=Math.max(...d.map(p=>p.eq));
  const X=i=>i/(d.length-1)*W, Y=e=>H-8-(e-lo)/((hi-lo)||1)*(H-30);
  let s=`<polyline fill="none" stroke="#e6b647" stroke-width="1.6" points="${d.map((p,i)=>X(i).toFixed(1)+','+Y(p.eq).toFixed(1)).join(' ')}"/>`;
  s+=`<text x="8" y="16" fill="#8ea0bd" font-size="10">equity ${fmt(d[d.length-1].eq)}</text>`;
  svg.innerHTML=s;
}

function kpi(s){
  if(!s)return;
  $('k_price').textContent=s.last_price?fmt(s.last_price):'–';
  $('k_poc').textContent=s.prev_poc!=null?fmt(s.prev_poc):'–';
  $('k_open').textContent=s.open0!=null?fmt(s.open0):'–';
  $('k_bias').textContent=s.bias==null?'–':(s.bias>0?'LONG':'SHORT');
  const ph=s.phase||'?';
  $('k_phase').textContent=ph;
  $('phasepill').innerHTML=ph.replaceAll('_',' ');
  const pc=ph==='in_position'?'y':ph==='bootstrap'?'b':ph==='waiting_trigger'?'g':'r';
  $('phasepill').innerHTML=`<span class="dot ${pc}"></span>${ph.replaceAll('_',' ')}`;
  $('k_atr').textContent=s.atr!=null?fmt(s.atr):'–';
  $('k_session').textContent=s.session||'–';
  if(s.balance&&s.balance.equity!=null)$('k_eq').textContent=fmt(s.balance.equity);
  if(s.last_bar)$('sub').textContent=`${s.source||''} · last bar ${new Date(s.last_bar.ts*1000).toISOString().replace('T',' ').slice(0,16)}Z · ${s.mode||''}`;
  $('mode').textContent=s.mode||'–';
  if(s.config){$('rr').textContent=s.config.rr;$('skip').textContent=s.config.skip_hour0?'on':'off';$('trail').textContent=s.config.trail_on?'on':'off';}
  // position panel
  const el=$('pos');
  if(s.position){
    const p=s.position;
    el.innerHTML=`<div class="win">${p.side.toUpperCase()} · @ ${fmt(p.entry)}</div>`+
      `<div>SL ${fmt(p.sl)} (orig ${fmt(p.sl0)}) · TP ${fmt(p.tp)}</div>`+
      `<div>stop ${(p.slp*100).toFixed(3)}% · RR ${p.rr}</div>`+
      `<div>run extreme ${fmt(p.runx)} · trail ${fmt(p.trail_a,2)}R</div>`+
      `<div>unrealized ${p.unreal_pct==null?'–':(p.unreal_pct>=0?'<span class="win">':'<span class="lose">')+p.unreal_pct.toFixed(3)+'%'}</span></div>`;
  }else if(s.phase==='waiting_trigger'){
    el.innerHTML='<div class="mut">flat — session traded: no</div>'+(s.prev_poc!=null?`<div>waiting first-touch of <span class="gold">PD PoC ${fmt(s.prev_poc)}</span> (${s.bias>0?'long pullback':'short rally'} bias)</div>`:'')+(s.open0!=null?`<div>session open ${fmt(s.open0)}</div>`:'');
  }else{el.innerHTML='<div class="mut">flat</div>';}
  // config
  if(s.config){$('cfg').textContent=JSON.stringify(s.config,null,1)+'\n'+JSON.stringify(s.balance||{},null,1);}
  // broker
  if(s.broker_status){$('broker').innerHTML=s.broker_status.slice().reverse().map(e=>`<div class="ev">${new Date(e.ts*1000).toISOString().slice(11,19)}Z <b>${e.msg}</b></div>`).join('')||'<span class="mut">no broker events</span>';}
  // trades
  if(s.last_trades&&s.last_trades.length>tradeCount){
    tradeCount=s.last_trades.length;
    const tb=$('trades').querySelector('tbody');
    s.last_trades.slice().reverse().forEach(t=>{
      const cls=t.pct>=0?'win':'lose';
      const row=document.createElement('tr');
      row.innerHTML=`<td>${t.session}</td><td>${tsUTC(t.entry_ts)}→${tsUTC(t.exit_ts)}</td><td>${t.session}</td>
      <td>${t.side}</td><td>${fmt(t.entry)}</td><td>${fmt(t.exit)}</td><td>${t.exit_type}</td>
      <td>${t.rr}</td><td class="${cls}">${t.pct>=0?'+':''}${t.pct.toFixed(3)}%</td><td class="${cls}">${t.r>=0?'+':''}${t.r.toFixed(2)}</td><td>${t.bars_held}</td>`;
      tb.prepend(row);
    });
    while(tb.children.length>40)tb.lastChild.remove();
  }
}

function addEvent(ev){
  const el=$('events');
  const d=new Date(ev.ts*1000).toISOString().slice(11,19)+'Z';
  let line;
  if(ev.type==='open')line=`${d} <b style="color:#57a7f0">OPEN</b> ${ev.data.side.toUpperCase()} @ ${fmt(ev.data.entry)} SL ${fmt(ev.data.sl)} TP ${fmt(ev.data.tp)}`;
  else if(ev.type==='close')line=`${d} <b>${ev.data.exit_type}</b> ${ev.data.side.toUpperCase()} @ ${fmt(ev.data.exit)} (${ev.data.pct>=0?'+':''}${ev.data.pct.toFixed(3)}%)`;
  else if(ev.type==='trail')line=`${d} <b style="color:#e6b647">TRAIL</b> ${ev.data.a}R → SL ${fmt(ev.data.sl)}`;
  else if(ev.type==='session')line=`${d} <b style="color:#3ec6d8">SESSION</b> ${ev.data.date} open ${fmt(ev.data.open)} bias ${ev.data.bias>0?'LONG':'SHORT'}`;
  else line=`${d} ${ev.type}`;
  const div=document.createElement('div');div.className='ev';div.innerHTML=line;
  el.prepend(div);
  while(el.children.length>60)el.lastChild.remove();
}

async function refresh(){
  try{
    const r=await fetch('/api/state');const s=await r.json();
    S.bars=s.recent_bars||S.bars;S.prof=s.prev_profile;S.pos=s.position;S.eq=s.equity_curve||S.eq;
    kpi(s);
    if(s.last_trades&&s.last_trades.length){if(!window._seen)s._seen=1;}
    drawChart();drawProfile();drawEquity();
    $('statusline').textContent=`connected · snapshot ${new Date().toISOString().slice(11,19)}Z · bars ${s.total_bars||S.bars.length} · trades ${(s.last_trades||[]).length}${window.sse?' · SSE live':' · polling'}`;
  }catch(e){$('statusline').textContent='offline — server not reachable (open via the running server URL)';}
}
function connectSSE(){
  try{
    const es=new EventSource('/api/events');
    es.onmessage=e=>{try{const ev=JSON.parse(e.data);if(ev.type==='state'){S.bars=ev.data.recent_bars||S.bars;S.prof=ev.data.prev_profile;S.pos=ev.data.position;S.eq=ev.data.equity_curve||S.eq;kpi(ev.data);drawChart();drawProfile();drawEquity();}else{addEvent(ev);}}catch(err){}};
    es.onerror=()=>{window.sse=false;};
  }catch(e){window.sse=false;}
}
refresh();setInterval(refresh,5000);connectSSE();
window.addEventListener('resize',()=>{drawChart();drawProfile();drawEquity();});
</script></body></html>"""
