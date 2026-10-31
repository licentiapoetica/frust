#!/usr/bin/env python3
"""One-off converter: flummpress templates (views/*.html) -> MiniJinja.

flummpress syntax:  {{ js }} (raw output)  {!! js !!} (escaped)  {{-- comment --}}
                    @if(js) @elseif(js) @else @endif  @each(js as v) @endeach
                    @include(path)  @mtime(/path)  @for(...) @endfor  @js ... @endjs

Output: Jinja with autoescape on. JS expressions are parsed and re-emitted as Jinja expressions.
Anything that can't be translated is kept as `PORT_TODO` markers (counted in the report) so it
can be fixed by hand.

Usage: convert_templates.py <views_dir> [--check]
"""
import os
import re
import sys

TODO = []


class Unsupported(Exception):
    pass


# ── Tokenizer ────────────────────────────────────────────────────────────────

PUNCT = ['===', '!==', '?.', '??', '...', '=>', '==', '!=', '<=', '>=', '&&', '||',
         '(', ')', '[', ']', '{', '}', ',', '.', '?', ':', '!', '+', '-', '*', '/', '%', '<', '>', '=']


def tokenize(s):
    toks = []
    i = 0
    n = len(s)
    while i < n:
        c = s[i]
        if c.isspace():
            i += 1
            continue
        if c in '\'"':
            j = i + 1
            buf = ''
            while j < n and s[j] != c:
                if s[j] == '\\' and j + 1 < n:
                    nx = s[j + 1]
                    buf += {'n': '\n', 't': '\t', 'r': '\r'}.get(nx, nx)
                    j += 2
                    continue
                buf += s[j]
                j += 1
            if j >= n:
                raise Unsupported('unterminated string')
            toks.append(('str', buf))
            i = j + 1
            continue
        if c == '`':
            j = i + 1
            parts = []
            buf = ''
            while j < n and s[j] != '`':
                if s[j] == '\\' and j + 1 < n:
                    buf += s[j + 1]
                    j += 2
                    continue
                if s[j] == '$' and j + 1 < n and s[j + 1] == '{':
                    depth = 1
                    k = j + 2
                    while k < n and depth:
                        if s[k] == '{':
                            depth += 1
                        elif s[k] == '}':
                            depth -= 1
                        k += 1
                    parts.append(('s', buf))
                    buf = ''
                    parts.append(('e', s[j + 2:k - 1]))
                    j = k
                    continue
                buf += s[j]
                j += 1
            parts.append(('s', buf))
            toks.append(('tpl', parts))
            i = j + 1
            continue
        if c.isdigit() or (c == '.' and i + 1 < n and s[i + 1].isdigit()):
            m = re.match(r'(0x[0-9a-fA-F]+|\d+(\.\d+)?([eE][+-]?\d+)?|\.\d+)', s[i:])
            toks.append(('num', m.group(0)))
            i += len(m.group(0))
            continue
        if c.isalpha() or c in '_$':
            m = re.match(r'[A-Za-z_$][A-Za-z0-9_$]*', s[i:])
            toks.append(('id', m.group(0)))
            i += len(m.group(0))
            continue
        for p in PUNCT:
            if s.startswith(p, i):
                # `?.` followed by a digit is a ternary with a number
                if p == '?.' and i + 2 < n and s[i + 2].isdigit():
                    continue
                toks.append(('p', p))
                i += len(p)
                break
        else:
            raise Unsupported(f'unexpected character {c!r}')
    toks.append(('eof', None))
    return toks


# ── AST ──────────────────────────────────────────────────────────────────────
# Nodes are tuples: ('lit', pyvalue) ('id', name) ('mem', obj, name) ('idx', obj, expr)
# ('call', callee, args) ('un', op, x) ('bin', op, a, b) ('cond', c, a, b) ('arr', items)
# ('obj', [(k, v)]) ('tpl', parts) ('new', callee, args) ('typeof', x) ('arrow', params, body)

BIN_PREC = [
    ['??'],
    ['||'],
    ['&&'],
    ['==', '!=', '===', '!=='],
    ['<', '>', '<=', '>=', 'in', 'instanceof'],
    ['+', '-'],
    ['*', '/', '%'],
]


class Parser:
    def __init__(self, src):
        self.t = tokenize(src)
        self.i = 0

    def peek(self, k=0):
        return self.t[self.i + k]

    def next(self):
        tok = self.t[self.i]
        self.i += 1
        return tok

    def accept(self, kind, val=None):
        tok = self.peek()
        if tok[0] == kind and (val is None or tok[1] == val):
            self.i += 1
            return tok
        return None

    def expect(self, kind, val=None):
        tok = self.accept(kind, val)
        if not tok:
            raise Unsupported(f'expected {val or kind}, got {self.peek()}')
        return tok

    def parse(self):
        e = self.expr()
        if self.peek()[0] != 'eof':
            raise Unsupported(f'trailing tokens {self.peek()}')
        return e

    def expr(self):
        # arrow function `x => ...` / `(a, b) => ...`
        if self.peek()[0] == 'id' and self.peek(1) == ('p', '=>'):
            name = self.next()[1]
            self.next()
            return ('arrow', [name], self.expr())
        if self.peek() == ('p', '('):
            j = self.i + 1
            params = []
            ok = True
            while self.t[j][0] == 'id':
                params.append(self.t[j][1])
                j += 1
                if self.t[j] == ('p', ','):
                    j += 1
                    continue
                break
            if ok and self.t[j] == ('p', ')') and self.t[j + 1] == ('p', '=>'):
                self.i = j + 2
                return ('arrow', params, self.expr())
        return self.ternary()

    def ternary(self):
        c = self.binary(0)
        if self.accept('p', '?'):
            a = self.expr()
            self.expect('p', ':')
            b = self.expr()
            return ('cond', c, a, b)
        return c

    def binary(self, lvl):
        if lvl >= len(BIN_PREC):
            return self.unary()
        left = self.binary(lvl + 1)
        while True:
            tok = self.peek()
            op = tok[1] if tok[0] in ('p', 'id') else None
            if op in BIN_PREC[lvl]:
                self.next()
                right = self.binary(lvl + 1)
                left = ('bin', op, left, right)
            else:
                return left

    def unary(self):
        if self.accept('p', '!'):
            return ('un', '!', self.unary())
        if self.accept('p', '-'):
            return ('un', '-', self.unary())
        if self.accept('p', '+'):
            return ('un', '+', self.unary())
        if self.accept('id', 'typeof'):
            return ('typeof', self.unary())
        if self.accept('id', 'new'):
            callee = self.primary(no_call=True)
            args = []
            if self.accept('p', '('):
                args = self.args()
            return self.postfix(('new', callee, args))
        return self.postfix(self.primary())

    def args(self):
        out = []
        if self.accept('p', ')'):
            return out
        while True:
            out.append(self.expr())
            if self.accept('p', ')'):
                return out
            self.expect('p', ',')

    def postfix(self, e):
        while True:
            if self.accept('p', '.') or self.accept('p', '?.'):
                if self.peek() == ('p', '('):
                    self.next()
                    e = ('call', e, self.args())
                    continue
                if self.peek() == ('p', '['):
                    self.next()
                    idx = self.expr()
                    self.expect('p', ']')
                    e = ('idx', e, idx)
                    continue
                name = self.expect('id')[1]
                e = ('mem', e, name)
            elif self.accept('p', '['):
                idx = self.expr()
                self.expect('p', ']')
                e = ('idx', e, idx)
            elif self.accept('p', '('):
                e = ('call', e, self.args())
            else:
                return e

    def primary(self, no_call=False):
        tok = self.next()
        k, v = tok
        if k == 'num':
            return ('lit', float(v) if ('.' in v or 'e' in v.lower()) and not v.startswith('0x') else int(v, 0))
        if k == 'str':
            return ('lit', v)
        if k == 'tpl':
            return ('tpl', v)
        if k == 'id':
            if v == 'true':
                return ('lit', True)
            if v == 'false':
                return ('lit', False)
            if v in ('null', 'undefined'):
                return ('lit', None)
            return ('id', v)
        if tok == ('p', '('):
            e = self.expr()
            self.expect('p', ')')
            return e
        if tok == ('p', '['):
            items = []
            if not self.accept('p', ']'):
                while True:
                    items.append(self.expr())
                    if self.accept('p', ']'):
                        break
                    self.expect('p', ',')
                    if self.accept('p', ']'):
                        break
            return ('arr', items)
        if tok == ('p', '{'):
            pairs = []
            if not self.accept('p', '}'):
                while True:
                    kt = self.next()
                    if kt[0] not in ('id', 'str', 'num'):
                        raise Unsupported('object key')
                    key = kt[1]
                    if self.accept('p', ':'):
                        val = self.expr()
                    else:
                        val = ('id', key)
                    pairs.append((key, val))
                    if self.accept('p', '}'):
                        break
                    self.expect('p', ',')
                    if self.accept('p', '}'):
                        break
            return ('obj', pairs)
        raise Unsupported(f'unexpected token {tok}')


# ── Emitter ──────────────────────────────────────────────────────────────────

def jstr(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n') + '"'


def lit(v):
    if v is None:
        return 'none'
    if v is True:
        return 'true'
    if v is False:
        return 'false'
    if isinstance(v, str):
        return jstr(v)
    return repr(v)


def is_stringy(n):
    return (n[0] == 'lit' and isinstance(n[1], str)) or n[0] == 'tpl' or (n[0] == 'bin' and n[1] == '+' and (is_stringy(n[2]) or is_stringy(n[3])))


def paren(s):
    return s if re.fullmatch(r'[A-Za-z_][A-Za-z0-9_.]*|"(?:[^"\\]|\\.)*"|-?\d+(\.\d+)?|\([^()]*\)', s) else f'({s})'


TYPEOF = {'undefined': 'undefined', 'string': 'string', 'number': 'number', 'boolean': 'boolean', 'object': 'mapping'}

GLOBAL_FUNCS = {
    'encodeURIComponent': lambda a: f'{paren(a[0])}|encode_uri_component',
    'encodeURI': lambda a: f'{paren(a[0])}|encode_uri',
    'parseInt': lambda a: f'parse_int({a[0]})',
    'parseFloat': lambda a: f'{paren(a[0])}|float',
    'Number': lambda a: f'{paren(a[0])}|float',
    'String': lambda a: f'{paren(a[0])}|string',
    'Boolean': lambda a: f'(true if {a[0]} else false)',
    'isNaN': lambda a: f'({paren(a[0])}|float != {paren(a[0])}|float)',
}


def emit(n):
    k = n[0]
    if k == 'lit':
        return lit(n[1])
    if k == 'id':
        name = n[1]
        if name == 'NaN':
            return '(0.0/0.0)'
        return name
    if k == 'tpl':
        parts = []
        for kind, val in n[1]:
            if kind == 's':
                if val:
                    parts.append(jstr(val))
            else:
                parts.append(paren(emit(Parser(val).parse())))
        return '(' + ' ~ '.join(parts or ['""']) + ')'
    if k == 'arr':
        return '[' + ', '.join(emit(x) for x in n[1]) + ']'
    if k == 'obj':
        return '{' + ', '.join(f'{jstr(str(key))}: {emit(v)}' for key, v in n[1]) + '}'
    if k == 'typeof':
        raise Unsupported('bare typeof')
    if k == 'un':
        op, x = n[1], n[2]
        if op == '!':
            if x[0] == 'un' and x[1] == '!':
                return f'(true if {emit(x[2])} else false)'
            return f'not {paren(emit(x))}'
        if op == '-':
            return f'-{paren(emit(x))}'
        return f'{paren(emit(x))}|float'
    if k == 'cond':
        return f'({emit(n[2])} if {emit(n[1])} else {emit(n[3])})'
    if k == 'bin':
        op, a, b = n[1], n[2], n[3]
        # typeof x === 'undefined' etc.
        for x, y in ((a, b), (b, a)):
            if x[0] == 'typeof' and y[0] == 'lit' and isinstance(y[1], str):
                t = TYPEOF.get(y[1])
                target = emit(x[1])
                neg = op in ('!==', '!=')
                if y[1] == 'function':
                    return 'true' if neg else 'false'
                if t is None:
                    raise Unsupported(f'typeof {y[1]}')
                if t == 'undefined':
                    return f'{paren(target)} is {"defined" if neg else "undefined"}'
                return f'{paren(target)} is {"not " if neg else ""}{t}'
        if op == '??':
            ea = emit(a)
            return f'({ea} if {paren(ea)} is defined and {paren(ea)} is not none else {emit(b)})'
        if op == '||':
            return f'({emit(a)} or {emit(b)})'
        if op == '&&':
            return f'({emit(a)} and {emit(b)})'
        if op in ('===', '=='):
            return f'{emit(a)} == {emit(b)}'
        if op in ('!==', '!='):
            return f'{emit(a)} != {emit(b)}'
        if op == '+':
            if is_stringy(a) or is_stringy(b):
                return f'{paren(emit(a))} ~ {paren(emit(b))}'
            return f'({emit(a)} + {emit(b)})'
        if op == 'in':
            return f'{paren(emit(a))} in {paren(emit(b))}'
        if op == 'instanceof':
            raise Unsupported('instanceof')
        if op == '/':
            return f'({emit(a)} / {emit(b)})'
        return f'({emit(a)} {op} {emit(b)})'
    if k == 'mem':
        obj, name = n[1], n[2]
        if name == 'length':
            return f'{paren(emit(obj))}|length'
        if obj == ('id', 'Math') and name == 'PI':
            return '3.141592653589793'
        return f'{paren(emit(obj))}.{name}' if re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*', name) else f'{paren(emit(obj))}[{jstr(name)}]'
    if k == 'idx':
        return f'{paren(emit(n[1]))}[{emit(n[2])}]'
    if k == 'new':
        raise Unsupported('new ' + str(n[1]))
    if k == 'arrow':
        raise Unsupported('arrow function')
    if k == 'call':
        return emit_call(n[1], n[2])
    raise Unsupported(k)


def date_arg(callee_obj):
    """`new Date(x)` -> emitted x (ms) or None."""
    if callee_obj[0] == 'new' and callee_obj[1] == ('id', 'Date'):
        if not callee_obj[2]:
            return 'now()'
        return emit(callee_obj[2][0])
    return None


def emit_call(callee, args):
    if callee[0] == 'id':
        name = callee[1]
        if name in GLOBAL_FUNCS:
            return GLOBAL_FUNCS[name]([emit(a) for a in args])
        return f'{name}(' + ', '.join(emit(a) for a in args) + ')'
    if callee[0] == 'mem':
        obj, m = callee[1], callee[2]
        ea = [emit(a) for a in args]
        # Static helpers
        if obj == ('id', 'JSON') and m == 'stringify':
            return f'{paren(ea[0])}|tojson'
        if obj == ('id', 'Math'):
            if m == 'floor':
                return f'floor({ea[0]})'
            if m == 'ceil':
                return f'ceil({ea[0]})'
            if m == 'round':
                return f'({paren(ea[0])}|round|int)'
            if m == 'abs':
                return f'{paren(ea[0])}|abs'
            if m in ('max', 'min') and len(ea) == 2:
                return f'{m}({ea[0]}, {ea[1]})'
        if obj == ('id', 'Date') and m == 'now':
            return 'now()'
        if obj == ('id', 'Array') and m == 'isArray':
            return f'{paren(ea[0])} is sequence'
        if obj == ('id', 'Object') and m == 'keys':
            return f'{paren(ea[0])}|list'
        if obj == ('id', 'Object') and m == 'values':
            return f'{paren(ea[0])}|values_of'
        if obj == ('id', 'Object') and m == 'entries':
            return f'{paren(ea[0])}|items'
        d = date_arg(obj)
        if d is not None:
            lang = f'({ea[0]})' if ea else ''
            if m == 'toLocaleString':
                return f'{paren(d)}|locale_string{lang}'
            if m == 'toLocaleDateString':
                return f'{paren(d)}|locale_date{lang}'
            if m == 'toLocaleTimeString':
                return f'{paren(d)}|locale_time{lang}'
            if m == 'toISOString':
                return f'{paren(d)}|iso_date'
            if m == 'getFullYear':
                return f'{paren(d)}|year'
            raise Unsupported('Date.' + m)
        o = paren(emit(obj))
        if m == 'includes':
            return f'({ea[0]} in {o})'
        if m == 'startsWith':
            return f'{o}.startswith({ea[0]})'
        if m == 'endsWith':
            return f'{o}.endswith({ea[0]})'
        if m == 'toLowerCase':
            return f'{o}|lower'
        if m == 'toUpperCase':
            return f'{o}|upper'
        if m == 'trim':
            return f'{o}|trim'
        if m == 'split' and len(ea) == 1:
            return f'{o}.split({ea[0]})'
        if m == 'join':
            return f'{o}|join({ea[0] if ea else jstr(",")})'
        if m == 'toString':
            return f'{o}|string'
        if m == 'toFixed':
            return f'{o}|fixed({ea[0] if ea else 0})'
        if m == 'toLocaleString':
            return f'{o}|locale_number'
        if m == 'charAt':
            return f'{o}[{ea[0]}]'
        if m in ('slice', 'substring'):
            if len(ea) == 1:
                return f'{o}[{ea[0]}:]'
            return f'{o}[{ea[0]}:{ea[1]}]'
        if m == 'substr':
            if len(ea) == 1:
                return f'{o}[{ea[0]}:]'
            return f'{o}[{ea[0]}:({ea[0]}) + ({ea[1]})]'
        if m == 'indexOf':
            return f'{o}.find({ea[0]})'
        if m == 'replace' and args and args[0][0] == 'lit' and isinstance(args[0][1], str):
            return f'{o}.replace({ea[0]}, {ea[1]}, 1)'
        if m == 'replaceAll' and args and args[0][0] == 'lit':
            return f'{o}.replace({ea[0]}, {ea[1]})'
        if m == 'repeat':
            return f'({o} * {ea[0]})'
        if m == 'padStart':
            raise Unsupported('padStart')
        if m in ('map', 'filter', 'some', 'every', 'find', 'reduce', 'forEach', 'sort'):
            raise Unsupported(m + ' with callback')
        if m == 'hasOwnProperty':
            return f'({ea[0]} in {o})'
        # t(), helpers on objects: keep as method call (pycompat may handle it)
        return f'{o}.{m}(' + ', '.join(ea) + ')'
    raise Unsupported('call on ' + callee[0])


def conv(js):
    js = js.strip()
    return emit(Parser(js).parse())


def conv_or_todo(js, where):
    try:
        return conv(js), True
    except Unsupported as e:
        TODO.append((where, js.strip(), str(e)))
        return None, False


# ── Template structure ───────────────────────────────────────────────────────

def balanced(text, start):
    """text[start] == '(' -> index after the matching ')' (skipping strings)."""
    depth = 0
    i = start
    quote = None
    while i < len(text):
        c = text[i]
        if quote:
            if c == '\\':
                i += 2
                continue
            if c == quote:
                quote = None
        elif c in '\'"`':
            quote = c
        elif c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    raise Unsupported('unbalanced parens')


SAFE_SUFFIXES = ('_json', '_html', '_b64', '_svg')


def output_tag(expr_js, ctx, where):
    """{{ js }} in the old engine printed raw. Choose the Jinja form for this context."""
    e, ok = conv_or_todo(expr_js, where)
    if not ok:
        return '{{ PORT_TODO }}{# PORT_TODO: ' + expr_js.strip().replace('#}', '# }') + ' #}'
    bare = expr_js.strip()
    name = re.sub(r'\W', '', bare.split('.')[-1].split('(')[0])
    if bare == '"@"' or bare == "'@'":
        return '@'
    if any(name.endswith(s) for s in SAFE_SUFFIXES) or re.fullmatch(r'\w+_html|svg', name):
        return '{{ ' + e + '|safe }}'
    if ctx == 'script':
        # Unquoted in a script the old engine emitted raw code (`= {{ x ? 'true' : 'false' }};`)
        return '{{ ' + e + '|safe }}'
    return '{{ ' + e + ' }}'


def convert(text, fname):
    out = []
    i = 0
    n = len(text)
    ctx = 'html'
    line = 1

    def where():
        return f'{fname}:{text.count(chr(10), 0, i) + 1}'

    while i < n:
        # track <script>/<style> context
        low = text[i:i + 8].lower()
        if low.startswith('<script'):
            ctx = 'script'
        elif low.startswith('</script'):
            ctx = 'html'
        elif low.startswith('<style'):
            ctx = 'style'
        elif low.startswith('</style'):
            ctx = 'html'

        if text.startswith('{{--', i):
            j = text.find('--}}', i)
            if j < 0:
                out.append(text[i:])
                break
            out.append('{#' + text[i + 4:j].replace('#}', '# }') + '#}')
            i = j + 4
            continue
        if text.startswith('{!!', i):
            j = text.find('!!}', i)
            e, ok = conv_or_todo(text[i + 3:j], where())
            out.append('{{ ' + (e if ok else 'PORT_TODO') + ' }}')
            i = j + 3
            continue
        if text.startswith('{{', i):
            j = text.find('}}', i + 2)
            if j < 0:
                out.append(text[i:])
                break
            expr = text[i + 2:j]
            # quoted value inside a script: '{{ x }}' -> {{ x|tojson }}
            if ctx == 'script' and out and out[-1].endswith(("'", '"')) and text[j + 2:j + 3] == out[-1][-1]:
                q = out[-1][-1]
                e, ok = conv_or_todo(expr, where())
                if ok:
                    out[-1] = out[-1][:-1]
                    out.append('{{ ' + e + '|string|tojson }}')
                    i = j + 3
                    continue
                _ = q
            out.append(output_tag(expr, ctx, where()))
            i = j + 2
            continue
        if text[i] == '@':
            m = re.match(r'@(if|elseif|each|include|mtime|for)\(', text[i:])
            if m:
                kw = m.group(1)
                p0 = i + len(m.group(0)) - 1
                try:
                    p1 = balanced(text, p0)
                except Unsupported:
                    out.append(text[i])
                    i += 1
                    continue
                inner = text[p0 + 1:p1 - 1]
                if kw in ('if', 'elseif'):
                    e, ok = conv_or_todo(inner, where())
                    tag = 'if' if kw == 'if' else 'elif'
                    out.append('{% ' + tag + ' ' + (e if ok else 'PORT_TODO') + ' %}' + ('' if ok else '{# PORT_TODO: ' + inner.replace('#}', '# }') + ' #}'))
                elif kw == 'each':
                    mm = re.match(r'(.*)\s+as\s+(\w+)\s*$', inner, re.S)
                    if not mm:
                        TODO.append((where(), inner, 'each syntax'))
                        out.append('{% for key, PORT_TODO in [] %}')
                    else:
                        e, ok = conv_or_todo(mm.group(1), where())
                        out.append('{% for key, ' + mm.group(2) + ' in ' + (paren(e) if ok else 'PORT_TODO') + '|entries %}')
                elif kw == 'include':
                    out.append('{% include "' + inner.strip() + '.html" %}')
                elif kw == 'mtime':
                    out.append('{{ mtime(' + jstr(inner.strip()) + ') }}')
                elif kw == 'for':
                    TODO.append((where(), inner, '@for loop'))
                    out.append('{# PORT_TODO @for(' + inner.replace('#}', '# }') + ') #}{% for PORT_TODO in [] %}')
                i = p1
                continue
            m = re.match(r'@(endif|endeach|endfor|endjs|else|js(?![A-Za-z0-9_]))', text[i:])
            if m:
                kw = m.group(1)
                out.append({'endif': '{% endif %}', 'endeach': '{% endfor %}', 'endfor': '{% endfor %}', 'else': '{% else %}'}.get(kw, ''))
                if kw == 'js':
                    j = text.find('@endjs', i)
                    TODO.append((where(), text[i + 3:j].strip()[:200], '@js block'))
                    out.append('{# PORT_TODO @js block:\n' + text[i + 3:j].replace('#}', '# }') + '\n#}')
                    i = j + len('@endjs')
                    continue
                i += len(m.group(0))
                continue
        # Literal Jinja delimiters in source text must not be interpreted
        if text.startswith('{%', i) or text.startswith('{#', i):
            out.append('{{ "' + text[i:i + 2] + '" }}')
            i += 2
            continue
        out.append(text[i])
        i += 1
    return ''.join(out)


def main():
    views = sys.argv[1]
    check = '--check' in sys.argv
    files = []
    for root, _, names in os.walk(views):
        for nm in names:
            if nm.endswith('.html'):
                files.append(os.path.join(root, nm))
    files.sort()
    for f in files:
        src = open(f, encoding='utf-8').read()
        if '{%' in src and '@if(' not in src and '{{' in src and 'PORT_' not in src and src.count('{%') > 3:
            continue  # already converted
        rel = os.path.relpath(f, views)
        res = convert(src, rel)
        if not check:
            open(f, 'w', encoding='utf-8').write(res)
    by_file = {}
    for w, js, why in TODO:
        by_file.setdefault(w.split(':')[0], []).append((w, js, why))
    for fn, items in sorted(by_file.items()):
        print(f'{fn}: {len(items)} TODO')
        for w, js, why in items:
            print(f'   {w}  [{why}]  {js[:140]!r}')
    print(f'\n{len(files)} files, {len(TODO)} expressions need manual porting')


if __name__ == '__main__':
    main()
