"""Invisible-character checker (our own; the user, 2026-10-14: "let no watermarks in and also clean older code").

AI-written text can carry characters you can't see: zero-width spaces/joiners, bidi controls (also the 'Trojan
Source' attack on code), Unicode tag characters, odd spaces, Hangul/Mongolian fillers, noncharacters. This removes
them from text files, keeping what real text needs: a ZWJ or variation selector inside an emoji sequence
(👨\u200d💻, ❤️), a byte-order mark at the very start of a file, and flag tag sequences after 🏴.

Enforced in: tools.write_text_file (on every write), office.continue_project (sweep of the project folder after
each part), and the project's git pre-commit hook.
    python invisible.py <folder or file> [--apply]     (dry run without --apply)
"""
import sys
import unicodedata
from pathlib import Path

TEXT_EXT = {".rs", ".toml", ".py", ".md", ".txt", ".json", ".ps1", ".psm1", ".bat", ".cmd", ".yml", ".yaml", ".html",
            ".htm", ".css", ".js", ".ts", ".tsx", ".jsx", ".go", ".java", ".kt", ".cs", ".c", ".h", ".cpp", ".hpp",
            ".sh", ".sql", ".xml", ".ini", ".cfg", ".env", ".csv", ".lock", ".gitignore", ".proto", ".rb"}
# third-party / generated code is not ours to change (vendored libraries legitimately contain these characters, e.g.
# emoji tables), and the app's data folder holds captured content, not code
SKIP_PARTS = {".git", "node_modules", "__pycache__", "agent_sources", "skill_quarantine", "data", ".venv", "venv",
              "env", "site-packages", "dist", "build", "vendor", "vendors", "third_party", "mitmproxy", ".gradle",
              ".idea", ".vs"}
SKIP_FILES = {"invisible.py"}  # this file defines the characters it removes
SPACES = {"\u00a0", "\u1680", "\u2000", "\u2001", "\u2002", "\u2003", "\u2004", "\u2005", "\u2006", "\u2007",
          "\u2008", "\u2009", "\u200a", "\u202f", "\u205f", "\u3000"}
FILLERS = {"\u115f", "\u1160", "\u180e", "\u180f", "\u3164", "\uffa0", "\u2800"}
KEEP_IN_EMOJI = {"\u200d"} | {chr(c) for c in range(0xFE00, 0xFE10)}


def _emojiish(ch):
    return bool(ch) and (ord(ch) >= 0x1F000 or unicodedata.category(ch) == "So" or 0x2190 <= ord(ch) <= 0x2BFF)


def clean_text(s, first_char_bom_ok=True):
    """(cleaned text, number of characters removed or replaced)."""
    out, n = [], 0
    for i, ch in enumerate(s):
        o = ord(ch)
        prev = out[-1] if out else ""
        nxt = s[i + 1] if i + 1 < len(s) else ""
        if ch in KEEP_IN_EMOJI and (_emojiish(prev) or prev in KEEP_IN_EMOJI) and (_emojiish(nxt) or ch != "\u200d"):
            out.append(ch)
            continue
        if ch == "\ufeff" and i == 0 and first_char_bom_ok:
            out.append(ch)
            continue
        if 0xE0020 <= o <= 0xE007F and out and ("\U0001F3F4" in out[-12:]):  # 🏴 subdivision flag tags
            out.append(ch)
            continue
        if ch in SPACES:
            out.append(" ")
            n += 1
        elif ch in ("\u2028", "\u2029"):
            out.append("\n")
            n += 1
        elif ch in FILLERS or 0xE0000 <= o <= 0xE0FFF or 0xFFF0 <= o <= 0xFFF8 or 0xFDD0 <= o <= 0xFDEF \
                or (o & 0xFFFE) == 0xFFFE or unicodedata.category(ch) == "Cf":
            n += 1  # zero-width / bidi / tag / format / noncharacter: dropped
        else:
            out.append(ch)
    return "".join(out), n


def is_text_file(p):
    p = Path(p)
    return p.suffix.lower() in TEXT_EXT or p.name in (".gitignore", ".gitattributes", "Dockerfile", "Makefile")


def clean_file(p, apply=True):
    """Characters removed from one file (written back only if apply and something changed)."""
    p = Path(p)
    if not is_text_file(p):
        return 0
    try:
        s = p.read_text(encoding="utf-8")
    except (UnicodeDecodeError, OSError):
        return 0
    t, n = clean_text(s)
    if n and apply:
        p.write_text(t, encoding="utf-8", newline="")
    return n


def clean_tree(root, apply=True, skip_parts=SKIP_PARTS):
    """{relative path: characters removed} for every text file under root (build folders and .git skipped)."""
    root = Path(root)
    if root.is_file():
        n = clean_file(root, apply)
        return {root.name: n} if n else {}
    found = {}
    for p in root.rglob("*"):
        parts = p.relative_to(root).parts[:-1]
        if any(x in skip_parts or x.startswith("target") for x in parts) or not p.is_file() or p.name in SKIP_FILES:
            continue
        n = clean_file(p, apply)
        if n:
            found[p.relative_to(root).as_posix()] = n
    return found


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    res = clean_tree(args[0] if args else ".", apply="--apply" in sys.argv)
    print(f"{'cleaned' if '--apply' in sys.argv else 'would clean'} {sum(res.values())} characters in "
          f"{len(res)} files")
    for f, n in sorted(res.items(), key=lambda x: -x[1])[:30]:
        print(f"  {n:5}  {f}")
    sys.exit(1 if res and "--check" in sys.argv else 0)
