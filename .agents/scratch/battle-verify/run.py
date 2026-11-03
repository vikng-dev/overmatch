"""Subject: the game's Battle cycle and damage-model credit, game state only.

Run with the machine lock held. The fixture client captures through the next fill-up;
no frame or tick cost instruments are armed. The deadline only bounds a stalled run.
"""
import datetime
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / '.agents/scratch/battle-verify'
env = dict(os.environ, OVERMATCH_UNATTENDED='1', OVERMATCH_WINDOW='hidden',
           BEVY_ASSET_ROOT=str(ROOT), RUST_BACKTRACE='1',
           OVERMATCH_CONFIG_DIR=str(OUT / 'config'),
           RUST_LOG='info,overmatch::net::score=debug')
processes = []
result = {'started_utc': datetime.datetime.now(datetime.timezone.utc).isoformat()}
try:
    with (OUT / 'server.log').open('w') as server_log, (OUT / 'client.log').open('w') as client_log:
        server = subprocess.Popen([str(ROOT / 'target/debug/overmatch-server')], cwd=ROOT,
            env=dict(env, OVERMATCH_SERVER_BIND='127.0.0.1', OVERMATCH_BOT='30'),
            stdin=subprocess.DEVNULL, stdout=server_log, stderr=subprocess.STDOUT)
        processes.append(server)
        for _ in range(120):
            if server.poll() is not None:
                raise RuntimeError('server exited during startup')
            if 'server: starting, listening' in (OUT / 'server.log').read_text():
                break
            time.sleep(0.5)
        else:
            raise RuntimeError('server did not announce listener')
        client = subprocess.Popen([str(ROOT / 'target/debug/overmatch')], cwd=ROOT,
            env=dict(env, OVERMATCH_SERVER='127.0.0.1:5888',
                     OVERMATCH_CAPTURE=str(OUT / 'cycle.ron')),
            stdin=subprocess.DEVNULL, stdout=client_log, stderr=subprocess.STDOUT)
        processes.append(client)
        (OUT / 'pids.json').write_text(json.dumps({'server': server.pid, 'client': client.pid}))
        deadline = time.monotonic() + 2700
        next_fill_up_at = None
        while client.poll() is None:
            if server.poll() is not None:
                raise RuntimeError('server exited during cycle')
            if time.monotonic() >= deadline:
                raise RuntimeError('cycle exceeded fixture watchdog')
            if 'server: Battle 2 FillUp' in (OUT / 'server.log').read_text():
                if next_fill_up_at is None:
                    next_fill_up_at = time.monotonic()
                if time.monotonic() - next_fill_up_at >= 12:
                    result['stopped_after_next_fill_up'] = True
                    break
            time.sleep(1)
        result['client_status'] = client.returncode
        if client.returncode is not None and client.returncode != 0:
            raise RuntimeError('client exited unsuccessfully')
        result['next_fill_up'] = 'server: Battle 2 FillUp' in (OUT / 'server.log').read_text()
        if not result['next_fill_up']:
            raise RuntimeError('client capture ended before the next fill-up')
except Exception as error:
    result['error'] = str(error)
finally:
    for process in reversed(processes):
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    result['statuses_after_cleanup'] = [p.returncode for p in processes]
    result['finished_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    (OUT / 'run-result.json').write_text(json.dumps(result, indent=2) + '\n')
