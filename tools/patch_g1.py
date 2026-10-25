import sys
base = r'D:\Projects\Ferrotap\src'

def load(p):
    raw = open(p, encoding='utf-8', newline='').read()
    crlf = '\r\n' in raw
    return raw.replace('\r\n', '\n'), crlf

def save(p, t, crlf):
    if crlf:
        t = t.replace('\n', '\r\n')
    open(p, 'w', encoding='utf-8', newline='').write(t)

def rep(t, old, new):
    if old not in t:
        print('PATTERN NOT FOUND:\n' + old); sys.exit(1)
    return t.replace(old, new, 1)

p = base + r'\lib.rs'
t, c = load(p)
if 'pub mod g1;' not in t:
    t = rep(t, 'pub mod codegen;\n', 'pub mod codegen;\npub mod g1;\n')
    save(p, t, c)

p = base + r'\ui.rs'
t, c = load(p)
if 'pending_copy' in t:
    print('already patched'); sys.exit(0)

t = rep(t, '''                if ui.button("Clear").clicked() {
                    self.model.apply(SessionEvent::Clear);
                }
''', '''                ui.menu_button("Clear \\u{25be}", |ui| {
                    for k in crate::g1::ClearKind::ALL_KINDS {
                        if ui.button(k.label()).clicked() {
                            if k == crate::g1::ClearKind::All {
                                self.model.apply(SessionEvent::Clear);
                            } else {
                                let n = self.model.clear_kind(k);
                                self.launch_msg = format!("Cleared {} session(s): {}", n, k.label());
                            }
                            ui.close();
                        }
                    }
                });
''')

t = rep(t, '        let mut pending_clear_comment: Option<usize> = None;\n',
        '        let mut pending_clear_comment: Option<usize> = None;\n'
        '        let mut pending_copy: Option<String> = None;\n'
        '        let mut pending_remove: Option<usize> = None;\n')

t = rep(t, '''                                pending_clear_comment = Some(s.id);
                                ui.close();
                            }
''', '''                                pending_clear_comment = Some(s.id);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Copy URL").clicked() { pending_copy = Some(s.url.clone()); ui.close(); }
                            if ui.button("Copy row (tab separated)").clicked() { pending_copy = Some(crate::g1::copy_row(s)); ui.close(); }
                            if ui.button("Copy summary").clicked() { pending_copy = Some(crate::g1::copy_summary(s)); ui.close(); }
                            if ui.button("Copy as cURL").clicked() { pending_copy = Some(crate::codegen::to_curl(s)); ui.close(); }
                            if ui.button("Copy as PowerShell").clicked() { pending_copy = Some(crate::codegen::to_powershell(s)); ui.close(); }
                            if ui.button("Copy as Fetch (JS)").clicked() { pending_copy = Some(crate::codegen::to_fetch(s)); ui.close(); }
                            if ui.button("Copy as Python (requests)").clicked() { pending_copy = Some(crate::codegen::to_python(s)); ui.close(); }
                            ui.separator();
                            if ui.button("Remove session").clicked() { pending_remove = Some(s.id); ui.close(); }
''')

t = rep(t, '        if let Some(id) = pending_clear_comment {\n',
        '''        if let Some(text) = pending_copy {
            ui.ctx().copy_text(text);
            self.launch_msg = "Copied to clipboard".to_string();
        }
        if let Some(id) = pending_remove {
            let n = self.model.remove_ids(&[id]);
            self.launch_msg = format!("Removed {} session", n);
        }
        if let Some(id) = pending_clear_comment {
''')
save(p, t, c)
print('patched')
