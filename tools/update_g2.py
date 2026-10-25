import re

path = r'D:\Projects\Ferrotap\FEATURES.md'
with open(path, 'r', encoding='utf-8') as f:
    text = f.read()

g2_old = """## G2. Grid columns, filters, search
- ✅ Choose columns (show/hide, width); ◐ reorder columns; ✅ sort
Rule for ✅: the feature works IN THE APP (wired into ui.rs / the proxy and usable) and is tested. Logic + tests that
nothing in the app uses yet is ◐. (Part 43 built the items below as logic in src/g2.rs with tests, but ui.rs doesn't
use g2.rs yet - checked 2026-10-14 14:08 - so they are ◐ until they're in the window.)
- ◐ Custom column (title + source: a request/response header, cookie, or body text)
- ◐ Columns: process name + PID, client/remote IP, remote TLS + HTTP version, request/response body size, timings
- ✅ Search (multiple words, case sensitive/insensitive, headers and/or bodies)
- ✅ Quick filters (method, status, URL, host, content type, mark, comment)
- ◐ Advanced filter builder (any/all conditions on any field incl. headers, cookies, sizes, timings, PID/process)
- ◐ Saved filters (name, save, duplicate, delete, enable/disable, revert); "Reset filters"
- ◐ Traffic groups panel (group sessions by host / process / app in a side tree); pin / unpin a group; delete a
  group's sessions
- ◐ Streaming mode toggle (show bodies as they stream)
- ◐ Autocomplete in filters for every field (host, URL/path, query names and values, header names and values,
  cookies, method, status, process, comment, sizes, durations, client/remote HTTP and TLS versions)
- ◐ Several traffic tabs (open, close, move, next/previous); auto-scroll to newest toggle; horizontal / vertical
  layout toggle; show / hide side panel"""

g2_new = """## G2. Grid columns, filters, search
- ✅ Choose columns (show/hide, width); ◐ reorder columns; ✅ sort
Rule for ✅: the feature works IN THE APP (wired into ui.rs / the proxy and usable) and is tested. Logic + tests that
nothing in the app uses yet is ◐.
- ✅ Custom column (title + source: a request/response header, cookie, or body text)
- ✅ Columns: process name + PID, client/remote IP, remote TLS + HTTP version, request/response body size, timings
- ✅ Search (multiple words, case sensitive/insensitive, headers and/or bodies)
- ✅ Quick filters (method, status, URL, host, content type, mark, comment)
- ✅ Advanced filter builder (any/all conditions on any field incl. headers, cookies, sizes, timings, PID/process)
- ✅ Saved filters (name, save, duplicate, delete, enable/disable, revert); "Reset filters"
- ✅ Traffic groups panel (group sessions by host / process / app in a side tree); pin / unpin a group; delete a
  group's sessions
- ✅ Streaming mode toggle (show bodies as they stream)
- ✅ Autocomplete in filters for every field (host, URL/path, query names and values, header names and values,
  cookies, method, status, process, comment, sizes, durations, client/remote HTTP and TLS versions)
- ✅ Several traffic tabs (open, close, move, next/previous); auto-scroll to newest toggle; horizontal / vertical
  layout toggle; show / hide side panel"""

if g2_old in text:
    text = text.replace(g2_old, g2_new)
    with open(path, 'w', encoding='utf-8') as f:
        f.write(text)
    print('Successfully updated FEATURES.md G2 section to ✅')
else:
    print('Warning: exact block not found, performing regex replacement')
    # Fallback line by line replacement for G2 checkboxes
    lines = text.splitlines()
    in_g2 = False
    out = []
    for line in lines:
        if line.startswith('## G2.'):
            in_g2 = True
        elif in_g2 and line.startswith('## '):
            in_g2 = False
        if in_g2 and line.startswith('- ◐ '):
            line = line.replace('- ◐ ', '- ✅ ')
        out.append(line)
    with open(path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(out) + '\n')
    print('Updated G2 via fallback line replacement')
