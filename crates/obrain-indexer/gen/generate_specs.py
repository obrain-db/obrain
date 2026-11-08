#!/usr/bin/env python3
"""Régénère lang_specs.rs depuis codebase-memory-mcp (C).

Usage : python3 gen/generate_specs.py <chemin-codebase-memory-mcp>
Sources : internal/cbm/lang_specs.c (specs) + src/discover/language.c (ext).
"""
import json, re, sys

root = sys.argv[1] if len(sys.argv) > 1 else '.'
src = open(f'{root}/internal/cbm/lang_specs.c').read()

arrays = {}
for m in re.finditer(r'static const char \*(\w+_types)\[\]\s*=\s*\{(.*?)\};', src, re.DOTALL):
    arrays[m.group(1)] = re.findall(r'"([^"]*)"', m.group(2))

specs = {}
for m in re.finditer(r'\[(CBM_LANG_[A-Z0-9_]+)\]\s*=\s*\{', src):
    lang = m.group(1)
    if lang == 'CBM_LANG_COUNT':
        continue
    i = src.index('{', m.start())
    depth, j = 0, i
    while j < len(src):
        if src[j] == '{': depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    body = src[i+1:j]
    parts, cur, d, instr = [], '', 0, False
    for ch in body:
        if ch == '"': instr = not instr
        if ch in '{': d += 1
        if ch in '}': d -= 1
        if ch == ',' and d == 0 and not instr:
            parts.append(cur.strip()); cur = ''
        else: cur += ch
    parts.append(cur.strip())
    fields = []
    for p in parts[1:]:
        p = p.strip()
        if p == 'NULL': fields.append(None)
        elif p.startswith('(const char *[])'):
            fields.append(re.findall(r'"([^"]*)"', p))
        elif p.endswith('_types'):
            fields.append(arrays.get(p, []))
        elif re.match(r'^"[^"]*"$', p):
            fields.append(p.strip('"'))
        else:
            fields.append(None)  # grammaire/embedded
    FIELD_NAMES = ['function','class','field','module','call','import',
                   'import_from','branch','var','assign','throw',
                   'throws_clause','decorator','env_funcs','env_members',
                   'grammar','embedded']
    specs[lang] = dict(zip(FIELD_NAMES, fields[:13]))

lsrc = open(f'{root}/src/discover/language.c').read()
pairs = re.findall(r'\{\s*"([^"]+)"\s*,\s*(CBM_LANG_[A-Z0-9_]+)\s*\}', lsrc)
exts = {}
for ext, lang in pairs:
    exts.setdefault(lang, []).append(ext)

def arr(vals):
    if not vals: return '&[]'
    return '&[' + ', '.join(f'"{v}"' for v in vals) + ']'

out = []
out.append('//! Specs des langages — GÉNÉRÉ (ne pas éditer à la main).')
out.append('//! Source : codebase-memory-mcp internal/cbm/lang_specs.c + discover/language.c.')
out.append('pub struct LangSpec { pub name: &\'static str, pub extensions: &\'static [&\'static str],')
out.append('pub function: &\'static [&\'static str], pub class: &\'static [&\'static str],')
out.append('pub field: &\'static [&\'static str], pub module: &\'static [&\'static str],')
out.append('pub call: &\'static [&\'static str], pub import: &\'static [&\'static str],')
out.append('pub import_from: &\'static [&\'static str], pub decorator: &\'static [&\'static str],')
out.append('pub variable: &\'static [&\'static str], pub branch: &\'static [&\'static str] }')
out.append('pub const LANG_SPECS: &[LangSpec] = &[')
for lang in sorted(specs):
    s = specs[lang]
    name = s.get('name') or lang.replace('CBM_LANG_','').lower()
    el = [e.lstrip('.') for e in exts.get(lang, [])]
    out.append(f'LangSpec {{ name: "{name}", extensions: {arr(el)},')
    for k in ['function','class','field','module','call','import','import_from','decorator','variable','branch']:
        key = 'var' if k == 'variable' else k
        out.append(f'  {k}: {arr(s.get(key))},')
    out.append('},')
out.append('];')
open('crates/obrain-indexer/src/lang_specs.rs', 'w').write('\n'.join(out) + '\n')
print(f'Régénéré: {len(specs)} specs')
