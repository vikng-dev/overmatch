"""A reader for the RON subset the tree assets are written in.

`assets/trees/species.ron` and every `<id>.tree.ron` are RON (the game's own format, read in Rust
by `ron`). This reader covers what those two files use: line and block comments, the
`#![enable(..)]` attribute line, strings, numbers, `true`/`false`, named structs `Name(a: 1)`,
tuple structs `Name(1, "x")`, anonymous tuples `(1, 2)`, anonymous structs `(a: 1)`, lists and
maps. It is not a RON implementation and refuses anything else by name.

Stdlib only, so the system interpreter (the build, the tests) and Blender's (the generator) both
load it.

Values map to Python as: string -> str, number -> int or float, bool -> bool, list -> list,
map -> dict, anonymous tuple -> tuple, anonymous or named struct -> dict (a named one under
`"__struct__": name`), tuple struct -> `(name, args)` as a `Call`.
"""

from __future__ import annotations

import re
from typing import Any, List, NamedTuple


class Call(NamedTuple):
    """A tuple struct such as `Unknown("why")`: its name and its positional arguments."""

    name: str
    args: tuple


class RonError(ValueError):
    """The text is outside the subset this reader covers, or is not well formed RON."""


_TOKEN = re.compile(r"""
    (?P<ws>\s+|//[^\n]*|/\*.*?\*/)
  | (?P<attr>\#!\[[^\]]*\])
  | (?P<string>"(?:\\.|[^"\\])*")
  | (?P<number>[+-]?(?:\d[\d_]*\.?[\d_]*|\.\d[\d_]*)(?:[eE][+-]?\d+)?)
  | (?P<ident>[A-Za-z_][A-Za-z0-9_]*)
  | (?P<punct>[()\[\]{}:,])
""", re.VERBOSE | re.DOTALL)


def _tokens(text: str) -> List[tuple]:
    out = []
    at = 0
    while at < len(text):
        match = _TOKEN.match(text, at)
        if match is None:
            raise RonError("unexpected character {!r} at offset {}".format(text[at], at))
        at = match.end()
        kind = match.lastgroup
        if kind in ("ws", "attr"):
            continue
        out.append((kind, match.group(kind)))
    return out


class _Parser:
    def __init__(self, text: str):
        self.tokens = _tokens(text)
        self.at = 0

    def peek(self, offset: int = 0):
        index = self.at + offset
        return self.tokens[index] if index < len(self.tokens) else (None, None)

    def take(self, kind: str = None, value: str = None):
        token = self.peek()
        if token[0] is None:
            raise RonError("unexpected end of text")
        if kind is not None and token[0] != kind:
            raise RonError("expected {}, found {!r}".format(kind, token[1]))
        if value is not None and token[1] != value:
            raise RonError("expected {!r}, found {!r}".format(value, token[1]))
        self.at += 1
        return token

    def value(self) -> Any:
        kind, text = self.peek()
        if kind == "string":
            self.take()
            return _unescape(text[1:-1])
        if kind == "number":
            self.take()
            return _number(text)
        if kind == "ident":
            self.take()
            if text == "true":
                return True
            if text == "false":
                return False
            if self.peek() == ("punct", "("):
                return self.call(text)
            raise RonError("bare identifier {!r} is outside the subset (only true/false and "
                           "`Name(..)` structs are read)".format(text))
        if (kind, text) == ("punct", "("):
            return self.group(None)
        if (kind, text) == ("punct", "["):
            return self.sequence("]")
        if (kind, text) == ("punct", "{"):
            return self.mapping()
        raise RonError("unexpected token {!r}".format(text))

    def call(self, name: str):
        return self.group(name)

    def group(self, name):
        """`( ... )` as a named struct, an anonymous struct, a tuple struct or a tuple."""
        self.take("punct", "(")
        named = self.peek()[0] == "ident" and self.peek(1) == ("punct", ":")
        if named:
            fields = {}
            while self.peek() != ("punct", ")"):
                key = self.take("ident")[1]
                self.take("punct", ":")
                fields[key] = self.value()
                if self.peek() == ("punct", ","):
                    self.take()
                elif self.peek() != ("punct", ")"):
                    raise RonError("expected , or ) after field {!r}".format(key))
            self.take("punct", ")")
            if name is not None:
                fields["__struct__"] = name
            return fields
        items = []
        while self.peek() != ("punct", ")"):
            items.append(self.value())
            if self.peek() == ("punct", ","):
                self.take()
            elif self.peek() != ("punct", ")"):
                raise RonError("expected , or ) in a tuple")
        self.take("punct", ")")
        if name is not None:
            return Call(name, tuple(items))
        return tuple(items)

    def sequence(self, close: str) -> list:
        self.take("punct", "[")
        items = []
        while self.peek() != ("punct", close):
            items.append(self.value())
            if self.peek() == ("punct", ","):
                self.take()
            elif self.peek() != ("punct", close):
                raise RonError("expected , or {} in a list".format(close))
        self.take("punct", close)
        return items

    def mapping(self) -> dict:
        self.take("punct", "{")
        out = {}
        while self.peek() != ("punct", "}"):
            key = self.value()
            self.take("punct", ":")
            out[key] = self.value()
            if self.peek() == ("punct", ","):
                self.take()
            elif self.peek() != ("punct", "}"):
                raise RonError("expected , or }} after key {!r}".format(key))
        self.take("punct", "}")
        return out


def _number(text: str):
    cleaned = text.replace("_", "")
    if re.fullmatch(r"[+-]?\d+", cleaned):
        return int(cleaned)
    return float(cleaned)


def _unescape(body: str) -> str:
    return body.encode("utf-8").decode("unicode_escape") if "\\" in body else body


def parse(text: str) -> Any:
    """One RON document as a Python value."""
    parser = _Parser(text)
    value = parser.value()
    if parser.peek()[0] is not None:
        raise RonError("trailing content after the document: {!r}".format(parser.peek()[1]))
    return value


def load(path: str) -> Any:
    with open(path, encoding="utf-8") as handle:
        return parse(handle.read())
