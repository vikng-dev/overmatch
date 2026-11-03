#!/usr/bin/env python3
"""Summarize the temporary native-collapse instrument; times are edit-profile observations."""
import json
from pathlib import Path
import statistics
import sys


def records(path, tag):
    marker=f'[DEBUG-rest] {tag} '
    return [json.loads(line.split(marker,1)[1]) for line in path.read_text().splitlines() if marker in line]


for path in map(Path,sys.argv[1:]):
    summary,wakes,bodies=[records(path,t) for t in ['summary','wake','bodies']]
    if not summary:
        continue
    tail=summary[-10:]
    result=dict(log=str(path),simulated_s=summary[-1]['tick']/64,wall_s=summary[-1]['wall_s'],
                tail_medians={key:statistics.median(row[key] for row in tail) for key in
                    ['awake','asleep','tick_us','physics_us','weigh_calls','weigh_us']},
                total_wakes={key:sum(row[key] for row in wakes) for key in
                    ['contact','force','position','rotation','velocity','timer','woken_bodies','slept_bodies']})
    if bodies:
        last=bodies[-1]
        awake=[b for b in last['bodies'] if b['piece'] and not b['sleep']]
        islands={}
        for body in awake:
            islands.setdefault(body['island'],[]).append(body)
        result['last_snapshot']=dict(tick=last['tick'],awake=len(awake),
            above_linear=sum(b['v']>=.15 for b in awake),above_angular=sum(b['w']>=.15 for b in awake),
            timer_mature=sum(b['timer']>=.5 for b in awake),
            islands=[dict(id=i,members=len(bs),above_angular=sum(b['w']>=.15 for b in bs),
                         mature=sum(b['timer']>=.5 for b in bs)) for i,bs in islands.items()])
    print(json.dumps(result))
