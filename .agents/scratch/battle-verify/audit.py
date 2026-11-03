"""Subject: the game's damage model, game state only. Read the captured authority log."""
from collections import Counter
import json
from pathlib import Path
import re

OUT = Path(__file__).resolve().parent
ROOT = OUT.parents[2]
markers = []
scores = []
spawns = []
phase = None
battle = None
for line in (OUT / 'server.log').read_text().splitlines():
    if match := re.search(r'server: Battle (\d+) (FillUp|Combat|Results)', line):
        battle, phase = int(match[1]), match[2]
    if 'server: knockout transition' in line:
        match = re.search(r'combatant (\d+) life (\d+) tank (\S+) at tick (\d+) reason (\w+)', line)
        assert match, line
        markers.append((battle, int(match[1]), int(match[2]), match[3], int(match[4])))
    if ' knocked out by ' in line:
        match = re.search(r'combatant (\d+) life (\d+) knocked out by (.*), turret (\w+) \((.*?)\), tank (\S+) at tick (\d+), cause (.*)', line)
        assert match, line
        scores.append(((battle, int(match[1]), int(match[2]), match[6], int(match[7])), match[5]))
    if 'spawned tank' in line and 'DEBUG overmatch::net::score:' in line:
        match = re.search(r'combatant (\d+) life (\d+) spawned tank (\S+) at tick (\d+)', line)
        assert match, line
        spawns.append((battle, phase, int(match[1]), int(match[2]), match[3], int(match[4])))

assert markers, 'the audit must observe real knockout transitions'
assert Counter(markers) == Counter(key for key, _ in scores), 'markers and score facts differ'
assert len({key[:3] for key in markers}) == len(markers), 'one marker transition per combatant life'
knockout_ticks = {(b, c, life): tick for b, c, life, _, tick in markers}
waits = []
for b, phase, c, life, _, tick in spawns:
    if phase == 'Combat' and life > 0:
        assert (b, c, life - 1) in knockout_ticks, 'a combat respawn follows its prior-life knockout'
        waits.append((tick - knockout_ticks[b, c, life - 1]) & 0xffffffff)
mode = (ROOT / 'assets/modes/battle.ron').read_text()
server = (ROOT / 'src/net/server.rs').read_text()
wait_seconds = float(re.search(r'respawn_wait_s:\s*([\d.]+)', mode)[1])
tick_ns = int(re.search(r'const TICK: Duration = Duration::from_nanos\(([\d_]+)\)', server)[1].replace('_', ''))
import math
required = math.ceil(wait_seconds * 1_000_000_000 / tick_ns)
assert waits and min(waits) >= required, 'every observed respawn respects the shipped wait'
credit_classes = Counter('Shooter' if credit.startswith('Shooter(') else credit for _, credit in scores)
server_log = (OUT / 'server.log').read_text()
client_log = (OUT / 'client.log').read_text()
summary = {
    'marker_transitions': len(markers), 'score_loss_entries': len(scores),
    'credit_classes': dict(credit_classes), 'marker_score_keys_equal': True,
    'combat_respawns': len(waits), 'required_wait_ticks_derived': required,
    'observed_wait_ticks': sorted(set(waits)),
    'server_panics': server_log.count('panicked at'), 'client_panics': client_log.count('panicked at'),
    'client_resync_warnings': client_log.count('FRONTIER interpolation resync #'),
    'results_observed': 'server: Battle 1 Results' in server_log,
    'next_fill_up_observed': 'server: Battle 2 FillUp' in server_log,
}
print(json.dumps(summary, indent=2))
