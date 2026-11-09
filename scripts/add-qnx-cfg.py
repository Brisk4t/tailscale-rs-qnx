#!/usr/bin/env python3
"""Add `target_os = "qnx"` (QNX SDP 8.0) next to every `target_os = "nto"` cfg in the given files.

- `cfg(target_os = "nto")` / `not(target_os = "nto")` become `...(any(target_os = "nto", target_os = "qnx"))`
- `target_os = "nto",` inside an existing any(...)/all(...) list gains a sibling `target_os = "qnx",`
- `all(target_os = "nto", ...)` (version-specific branches) is left alone and reported: decide by hand.
Idempotent: files already mentioning `target_os = "qnx"` next to nto are not changed twice.
"""
import re
import sys

NTO = 'target_os = "nto"'
QNX = 'target_os = "qnx"'

def patch(text):
    manual = []
    # Single-predicate wrappers: cfg(...), cfg_attr(..., ), not(...)
    text = re.sub(r'\b(cfg|not|cfg_attr)\(\s*target_os = "nto"\s*([,)])',
                  lambda m: f'{m.group(1)}(any({NTO}, {QNX}){m.group(2)}', text)
    out = []
    for i, line in enumerate(text.splitlines(keepends=True), 1):
        if NTO in line and QNX not in line:
            if re.search(r'all\(\s*target_os = "nto"', line):
                manual.append((i, line.strip()))
            elif re.fullmatch(r'(\s*)target_os = "nto",?\s*\n?', line):
                indent = re.match(r'\s*', line).group(0)
                if not line.rstrip().endswith(','):
                    line = line.rstrip('\n') + ',\n'
                out.append(line)
                line = f'{indent}{QNX},\n'
            else:
                line = line.replace(NTO + ',', f'{NTO}, {QNX},').replace(NTO + ')', f'{NTO}, {QNX})')
        out.append(line)
    return ''.join(out), manual

for path in sys.argv[1:]:
    src = open(path).read()
    new, manual = patch(src)
    if new != src:
        open(path, 'w').write(new)
        print(f'patched {path}')
    for ln, l in manual:
        print(f'  MANUAL {path}:{ln}: {l}')
