#!/usr/bin/env python3
"""Summarize MEASURED period p50s and DERIVED non-wait inclusive sample shares."""
import collections
import json
from pathlib import Path
import re
import statistics
import subprocess
import sys

GROUPS = {
    'apply_deposits': ['breakage::apply_deposits'],
    'weigh': ['fracture::weigh'],
    'structural': ['fracture::structural'],
    'loose_groups': ['fracture::loose_groups'],
    'incident': ['fracture::incident'],
    'held': ['fracture::held'],
    'narrow_phase': ['narrow_phase::'],
    'solver': ['avian3d::dynamics::solver'],
    'child_pose': ['ChildColliderPoses', 'update_child_collider_position'],
    'transforms': ['mark_dirty_trees', 'propagate_parent_transforms', 'sync_simple_transforms',
                   'position_to_transform', 'transform_to_position'],
    'contact_transfer': ['contact_transfer::'],
    'aabb': ['update_moved_collider_aabbs'],
    'collider_propagation': ['propagate_collider_transforms'],
    'scheduler': ['bevy_ecs::schedule::executor', 'bevy_tasks::', 'async_executor::'],
}
WAIT = ['__psynch_cvwait', '__psynch_mutexwait', '__semwait_signal', 'semaphore_wait_trap',
        'semaphore_timedwait_trap', 'mach_msg2_trap', '__workq_kernreturn', 'kevent',
        '__ulock_wait', '__ulock_wait2']


def sample(path):
    raw = path.read_text()
    raw = subprocess.run(['/usr/bin/c++filt'], input=raw, text=True,
                         capture_output=True, check=True).stdout
    graph = raw.split('Call graph:', 1)[1].split('Total number in stack', 1)[0]
    nodes, stack = [], []
    for line in graph.splitlines():
        match = re.match(r'^([ +!|:`]*)(\d+) (.+)$', line)
        if not match:
            continue
        prefix, count, symbol = match.groups()
        depth = len(prefix)
        while stack and nodes[stack[-1]]['depth'] >= depth:
            stack.pop()
        node = dict(depth=depth, count=int(count), own=int(count), symbol=symbol,
                    ancestors=[nodes[i]['symbol'] for i in stack])
        if stack:
            nodes[stack[-1]]['own'] -= int(count)
        stack.append(len(nodes))
        nodes.append(node)
    counts = collections.Counter()
    leaves = collections.Counter()
    for node in nodes:
        weight = node['own']
        if weight <= 0:
            continue
        frames = node['ancestors'] + [node['symbol']]
        counts['total_thread_samples'] += weight
        if any(any(w in s for w in WAIT) for s in frames):
            counts['wait_samples'] += weight
            continue
        counts['active_samples'] += weight
        if any(needle in node['symbol'] for needle in ['__psynch_cvsignal', 'bevy_ecs::schedule::executor', 'bevy_tasks::', 'async_executor::', '_pthread_cond_signal']):
            counts['scheduler_self'] += weight
        for name, needles in GROUPS.items():
            if any(any(needle in s for needle in needles) for s in frames):
                counts[name] += weight
        leaves[node['symbol'].split('  (in ')[0]] += weight
    return dict(counts=counts, inclusive_pct={k: round(100*v/counts['active_samples'], 2)
                for k, v in counts.items() if k in GROUPS or k == 'scheduler_self'},
                top_self=leaves.most_common(15))


def periods(rows):
    med = lambda fn: round(statistics.median(fn(r) for r in rows), 4)
    return dict(periods=len(rows), awake=med(lambda r:r['debris']['live']),
                asleep=med(lambda r:r['debris']['sleeping']),
                tick_ms=med(lambda r:r['tick_us']['p50']/1000),
                physics_ms=med(lambda r:r['physics_us']['p50']/1000),
                send_ms=med(lambda r:r['send_us']['p50']/1000),
                hz=med(lambda r:r['tick_hz']), cpu_pct=med(lambda r:r['cpu_pct']),
                entities=med(lambda r:r['entities']))


def main():
    for directory in map(Path, sys.argv[1:]):
        run = json.loads((directory/'result.json').read_text())
        rows = run['rows']
        elapsed = 0
        for row in rows:
            elapsed += row['period_s']
            row['end_s'] = row.get('wall_s', elapsed)
        summary = dict(label=run['label'], profile=run['profile'],
                       tail=periods(rows[-20:]) if rows else None, samples=[])
        for entry in run['samples']:
            lo, hi = entry['window_s']
            selected = [r for r in rows if lo <= r['end_s'] <= hi+1]
            summary['samples'].append(dict(window_s=[lo, hi],
                stats=periods(selected) if selected else None,
                profile=sample(Path(entry['path']))))
        (directory/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
        print(json.dumps(summary))


if __name__ == '__main__':
    main()
