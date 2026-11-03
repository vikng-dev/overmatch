#!/usr/bin/env python3
"""Video-game environment destruction/idle instrument; all measurements take the machine lock.
Build separately through scripts/serial-cargo, with dev_ui,bitprobe. Raw logs stay in target/.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('label')
    p.add_argument('--profile', default='profiling')
    p.add_argument('--client-profile', default='debug')
    p.add_argument('--variant', default='opt3 ThinLTO; game CGU16/debug1; release dependencies')
    p.add_argument('--duration', type=float, default=90)
    p.add_argument('--shells', type=int, default=0)
    p.add_argument('--period', type=float, default=1)
    p.add_argument('--plan', type=Path)
    p.add_argument('--samples', default='15,45,75')
    p.add_argument('--holding-lock', action='store_true')
    args = p.parse_args()
    if not args.holding_lock:
        os.execv('/usr/bin/lockf', ['lockf', '-k', '-t', '10800',
                 '/tmp/overmatch-cargo.lock', sys.executable, __file__,
                 *sys.argv[1:], '--holding-lock'])
    out = ROOT / 'target/slowdown-hunt' / args.label
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, OVERMATCH_UNATTENDED='1', OVERMATCH_WINDOW='headless',
               OVERMATCH_MUTE='1', BEVY_ASSET_ROOT=str(ROOT), RUST_BACKTRACE='1',
               OVERMATCH_SERVER_BIND='127.0.0.1', OVERMATCH_SERVER='127.0.0.1:5888',
               OVERMATCH_BOT='0', OVERMATCH_SERVER_STATS_S='1', OVERMATCH_DEBRIS_STRESS='off',
               OVERMATCH_HE_STRESS=f'{args.shells}:{args.period}')
    for key in ['OVERMATCH_SERVER_ID', 'OVERMATCH_NETCODE_KEY', 'OVERMATCH_REPORT_SECRET',
                'OVERMATCH_API', 'OVERMATCH_CAPTURE', 'OVERMATCH_HARNESS', 'SPIKE_COST_TRACE',
                'SPIKE_TRACE', 'SPIKE_FRAME_COST', 'SPIKE_RENDER_COST']:
        env.pop(key, None)
    if args.plan:
        env['OVERMATCH_HARNESS'] = str(args.plan.resolve())
        env['SPIKE_TRACE'] = str(out / 'trace')
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(('127.0.0.1', 5888))
    processes, files, samples = [], [], []
    reader = None
    deadlines = [float(x) for x in args.samples.split(',') if x]
    start = time.monotonic()
    result = dict(label=args.label, profile=args.profile, client_profile=args.client_profile, variant=args.variant, duration=args.duration,
                  shells=args.shells, period=args.period, samples=[], rows=[])
    try:
        for binary, role in [('overmatch-server', 'server')] + ([('overmatch', 'client')] if args.plan else []):
            log = (out / f'{role}.log').open('w')
            files.append(log)
            if role == 'server':
                reader = (out / 'server.log').open()
            processes.append(subprocess.Popen([str(ROOT / 'target' / (args.profile if role == 'server' else args.client_profile) / binary)],
                             cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT))
            if role == 'server' and args.plan:
                time.sleep(4)
        while time.monotonic() - start < args.duration:
            elapsed = time.monotonic() - start
            for line in reader.readlines():
                if line.startswith('server_stats '):
                    row = json.loads(line.removeprefix('server_stats '))
                    row['wall_s'] = elapsed
                    result['rows'].append(row)
            if args.plan and processes[1].poll() not in (None, 0):
                raise RuntimeError(f'client failed: {processes[1].returncode}; see {out}')
            if processes[0].poll() is not None:
                raise RuntimeError(f'server exited: {processes[0].returncode}; see {out}')
            if deadlines and elapsed >= deadlines[0]:
                at = deadlines.pop(0)
                dest = out / f'sample-{at:g}.txt'
                sample_log = (out / f'sample-{at:g}.stderr').open('w')
                files.append(sample_log)
                samples.append(subprocess.Popen(['/usr/bin/sample', str(processes[0].pid), '4', '1', '-mayDie',
                               '-file', str(dest)], stdout=subprocess.DEVNULL, stderr=sample_log))
                result['samples'].append(dict(window_s=[elapsed, elapsed + 4], path=str(dest)))
            time.sleep(.1)
        result['observed_status'] = [p.poll() for p in processes]
        result['trailing_censored_s'] = time.monotonic() - start - result['rows'][-1]['wall_s'] if result['rows'] else None
        if not result['rows'] or result['trailing_censored_s'] > 1.1:
            raise RuntimeError('incomplete trailing stats window')
        if args.plan and processes[1].poll() != 0:
            raise RuntimeError('client did not complete its script')
        for sample in samples:
            sample.wait(timeout=10)
            if sample.returncode:
                raise RuntimeError('sample failed')
    except BaseException as error:
        result['failure'] = str(error)
        raise
    finally:
        for sample in samples:
            if sample.poll() is None:
                sample.kill()
            sample.wait()
        for proc in reversed(processes):
            if proc.poll() is None:
                proc.send_signal(signal.SIGINT)
        for proc in processes:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        for log in files:
            log.close()
        result['elapsed_s'] = time.monotonic() - start
        if reader:
            reader.close()
        result['final_status'] = [p.returncode for p in processes]
        (out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({k: v for k, v in result.items() if k != 'rows'}), flush=True)


if __name__ == '__main__':
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f'signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    main()
