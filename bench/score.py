"""Score bench.jsonl with the rule pack's own detectors.

A tell is a distinct span of text flagged by any of: the pack's 20 regex
detectors, its tier-1 vocabulary, or the phrases it knows are slop but has no
safe substitute for. Overlapping spans are merged, so a phrase that trips two
detectors counts once.

Run from the repo root:  python score.py bench.jsonl
"""
import json
import re
import statistics
import sys
from collections import defaultdict

pack = json.load(open('rules/slop-rules.json', encoding='utf-8'))

DETECTORS = []
SKIPPED = []

for r in pack['regex']:
    flags = re.M | (re.I if 'i' in r.get('flags', '') else 0)
    try:
        DETECTORS.append((r['id'], re.compile(r['pattern'], flags), r['weight']))
    except re.error as e:
        SKIPPED.append(r['id'])

for w in pack['vocabulary']['tier1']['words']:
    DETECTORS.append(('vocab:' + w, re.compile(r'\b' + re.escape(w) + r'\b', re.I), 3))

for p in pack['phrases']:
    if not p.get('fix'):
        DETECTORS.append(('phrase:' + p['match'],
                          re.compile(r'\b' + re.escape(p['match']) + r'\b', re.I), 2))

# Two shapes the system prompt bans by name that the pack's regexes miss. The
# pack's negative-parallelism pattern requires a following "but", so the
# "doesn't just X, it Y" variant the prompt calls out slips through; and the
# pack covers "in today's ..." openers but not "in the world of ...". Scoring
# these is scoring what the prompt actually asked the model to do.
DETECTORS.append((
    'prompt:not-just-it',
    re.compile(r"(?:not|n[o’']t)\s+(?:just|only|merely)\b[^.!?\n]{0,80}?,\s*(?:it|this|that|they)\b", re.I),
    3))
DETECTORS.append((
    'prompt:scene-setting',
    re.compile(r"(?:^|\n|\. )\s*in (?:the world of|a world of|an era|the era|an age of|the age of)\b", re.I),
    3))


def tells(text):
    """(distinct flagged spans, weighted score, detector ids that hit)."""
    spans, weight, hits = [], 0, []
    for id_, rx, w in DETECTORS:
        for m in rx.finditer(text):
            if m.end() > m.start():
                spans.append((m.start(), m.end()))
                weight += w
                hits.append(id_)
    spans.sort()
    merged, end = 0, -1
    for s, e in spans:
        if s >= end:
            merged += 1
            end = e
        else:
            end = max(end, e)
    return merged, weight, hits


def main(path):
    rows = [json.loads(l) for l in open(path, encoding='utf-8') if l.strip()]
    by_model = defaultdict(list)
    for r in rows:
        by_model[r['model']].append(r)

    if SKIPPED:
        print('detectors skipped (pattern unsupported in Python):', ', '.join(SKIPPED))
    print(f'{len(DETECTORS)} detectors, {len(rows)} rewrites scored\n')

    # Input and rules-only baseline are identical for every model, so they are
    # scored once and stand as the two reference columns.
    first = by_model[next(iter(by_model))]
    per_id = {}
    for r in first:
        per_id.setdefault(r['id'], (tells(r['input'])[0], tells(r['baseline'])[0]))
    in_total = sum(v[0] for v in per_id.values())
    base_total = sum(v[1] for v in per_id.values())
    print(f'corpus: {len(per_id)} samples, {in_total} tells raw, '
          f'{base_total} left by the rules pass alone (the baseline every model must beat)\n')

    hdr = (f"{'model':<24}{'tells/run':>10}{'vs rules':>10}{'rejected':>11}"
           f"{'facts lost':>12}{'median s':>10}{'load':>8}{'VRAM':>9}")
    print(hdr)
    print('-' * len(hdr))

    detail = {}
    for name, rs in by_model.items():
        runs = max(r['run'] for r in rs) + 1
        counts, lost, rejects, ms, hitbag = [], 0, 0, [], defaultdict(int)
        for r in rs:
            c, _w, hits = tells(r['output'])
            counts.append(c)
            for h in hits:
                hitbag[h] += 1
            if r['rejected']:
                rejects += 1
            for f in (r['facts'] or []):
                if f not in r['output']:
                    lost += 1
            ms.append(r['ms'])
        per_run = sum(counts) / runs
        drop = 100 * (1 - per_run / base_total) if base_total else 0
        print(f"{name:<24}{per_run:>10.1f}{drop:>9.0f}%{rejects:>7}/{len(rs):<3}"
              f"{lost:>12}{statistics.median(ms) / 1000:>10.2f}"
              f"{rs[0]['load_ms'] / 1000:>7.1f}s{rs[0]['vram_mib']:>8}M")
        detail[name] = (hitbag, rs, counts)

    print('\nper-sample tells left (rules-only baseline in brackets)')
    ids = list(per_id)
    print(f"  {'sample':<18}{'base':>6}" + ''.join(f'{n[:14]:>16}' for n in by_model))
    for i in ids:
        line = f'  {i:<18}{per_id[i][1]:>6}'
        for name, rs in by_model.items():
            vals = [tells(r['output'])[0] for r in rs if r['id'] == i]
            line += f'{statistics.mean(vals):>16.1f}'
        print(line)

    print('\ntells that survive most often')
    for name, (hitbag, _rs, _c) in detail.items():
        top = sorted(hitbag.items(), key=lambda kv: -kv[1])[:6]
        print(f'  {name:<24}' + (', '.join(f'{k} x{v}' for k, v in top) or 'none'))

    print('\nrejection reasons')
    for name, (_h, rs, _c) in detail.items():
        why = defaultdict(int)
        for r in rs:
            if r['rejected']:
                why[r['rejected']] += 1
        print(f'  {name:<24}' + (', '.join(f'{k} x{v}' for k, v in sorted(why.items(), key=lambda kv: -kv[1])) or 'none'))


if __name__ == '__main__':
    main(sys.argv[1])
