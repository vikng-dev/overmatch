"""Video-game environment-physics reproducer; diagnostic run bounds, no cost sampling.
The report owns the measured result. Run in the leased bay after building the selected source.
"""
import os,subprocess,time,signal,json,sys,pathlib
if '--holding-lock' not in sys.argv:
 os.execv('/usr/bin/lockf',['lockf','-k','-t','10800','/tmp/overmatch-cargo.lock',sys.executable,__file__,'--holding-lock',*sys.argv[1:]])
sys.argv.remove('--holding-lock')
root=pathlib.Path.cwd(); out=root/'target/diag-tree-crash'; label=sys.argv[1]; plan=pathlib.Path(sys.argv[2]).resolve()
env=dict(os.environ,OVERMATCH_UNATTENDED='1',OVERMATCH_WINDOW='headless',OVERMATCH_MUTE='1',RUST_BACKTRACE='1',BEVY_ASSET_ROOT=str(root),OVERMATCH_SERVER_BIND='127.0.0.1',OVERMATCH_SERVER='127.0.0.1:5888',OVERMATCH_HARNESS=str(plan),SPIKE_TRACE=str(out/label),RUST_LOG='info,overmatch::net::server=debug')
out.mkdir(parents=True,exist_ok=True)
for k in ['OVERMATCH_SERVER_ID','OVERMATCH_NETCODE_KEY','OVERMATCH_REPORT_SECRET','SPIKE_COST_TRACE','SPIKE_FRAME_COST','SPIKE_RENDER_COST','OVERMATCH_SERVER_STATS_S','OVERMATCH_API','OVERMATCH_CAPTURE','OVERMATCH_BOT']:
 env.pop(k,None)
procs=[]; logs=[]; result={'label':label,'plan':plan.read_text()}; start=time.monotonic()
try:
 for binary,role in [('overmatch-server','server'),('overmatch','client')]:
  env['OVERMATCH_WINDOW']='headless' if role=='server' else os.environ.get('DIAG_WINDOW','hidden')
  log=open(out/f'{label}.{role}.log','w'); logs.append(log)
  p=subprocess.Popen([str(root/'target/debug'/binary)],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT); procs.append(p)
  if role=='server':time.sleep(4)
 while time.monotonic()-start<175 and all(p.poll() is None for p in procs):time.sleep(.2)
 result['observed_status']=[p.poll() for p in procs]; result['elapsed_s']=time.monotonic()-start
finally:
 for p in reversed(procs):
  if p.poll() is None:p.send_signal(signal.SIGINT)
 for p in procs:
  try:p.wait(timeout=10)
  except subprocess.TimeoutExpired:p.kill();p.wait()
 for log in logs:log.close()
 result['final_status']=[p.returncode for p in procs]
 (out/f'{label}.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)

if result["observed_status"][0] is not None: sys.exit(1)
if result["observed_status"][1] != 0: sys.exit(2)
