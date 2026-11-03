import json,re,statistics,datetime,pathlib,collections,sys
root=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path(__file__).parent
ansi=re.compile(r'\x1b\[[0-9;]*m'); date=re.compile(r'^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+Z)')
def server(path):
 lines=[ansi.sub('',x) for x in path.read_text().splitlines()];previous=[];t=None
 for l in lines:
  if m:=date.match(l):t=datetime.datetime.fromisoformat(m[1]).timestamp()
  previous.append(t)
 following=[None]*len(lines);t=None
 for i in range(len(lines)-1,-1,-1):
  if m:=date.match(lines[i]):t=datetime.datetime.fromisoformat(m[1]).timestamp()
  following[i]=t
 rows=[]
 for i,l in enumerate(lines):
  if l.startswith('server_stats '):
   d=json.loads(l[13:]);ends=[x for x in [previous[i],following[i]] if x is not None];d['at']=sum(ends)/len(ends);rows.append(d)
 return rows
def summary(rows):
 if not rows:return None
 med=lambda f:round(statistics.median(f(d) for d in rows),3)
 return {'periods':len(rows),'awake':med(lambda d:d['debris']['live']),'asleep':med(lambda d:d['debris']['sleeping']),'speed':med(lambda d:d['tick_hz']/64),'tick_ms':med(lambda d:d['tick_us']['p50']/1000),'physics_ms':med(lambda d:d['physics_us']['p50']/1000),'send_ms':med(lambda d:d['send_us']['p50']/1000),'rss_mb':med(lambda d:d['rss_mb'])}
path=root/'remote-spread-final.log'
r=server(path if path.exists() else root/'remote-spread.log')
fires=[d for l in (root/'spread-shot.client.jsonl').read_text().splitlines() if (d:=json.loads(l))['k']=='fire_rx' and not d.get('dup')]
result={'method':'MEASURED server stats period p50 medians; DERIVED log timestamp bracket midpoints and speed=Hz/64','baseline':summary([d for d in r if fires[0]['us']/1e6-31<d['at']<fires[0]['us']/1e6-1]),'shots':[]}
for i,f in enumerate(fires):
 t=f['us']/1e6;sel=[d for d in r if t+1<d['at']<t+4.4];result['shots'].append({'shot':i+1,'type':'initial APHE' if i==0 else 'HE','fire_tick':f['ft'],'arrival_utc':datetime.datetime.fromtimestamp(t,datetime.timezone.utc).isoformat(),'stats':summary(sel)})
left=datetime.datetime(2026,11,3,14,37,11,884000,tzinfo=datetime.timezone.utc).timestamp()
result['empty_1m']=summary([d for d in r if left+45<d['at']<left+75]);result['empty_10m']=summary([d for d in r if left+585<d['at']<left+615]);result['empty_observed_s']=max(d['at'] for d in r if d['clients']==0)-left
old=server(root/'remote-he.log'); result['same_opening']={'max_pieces':max(d['debris']['live']+d['debris']['sleeping'] for d in old),'empty_pile':summary([d for d in old if d['clients']==0 and d['debris']['live']+d['debris']['sleeping']>0][-30:]),'last':summary(old[-30:])}
result['local']={}
for path in sorted(root.glob('*-*.log')):
 ss=[json.loads(l[21:]) for l in path.read_text(errors='replace').splitlines() if l.startswith('[DEBUG-rest] summary ')]
 if not ss:continue
 local={}
 for label,a,b in [('baseline',40,50),('after',51,61),('late',90,100)]:
  sel=[d for d in ss if a*64<=d['tick']<=b*64]
  if sel:local[label]={k:round(statistics.median(d[k] for d in sel),3) for k in ['awake','asleep','tick_us','physics_us','weigh_calls','weigh_us']}
 result['local'][path.name]=local
(root/'summary.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
