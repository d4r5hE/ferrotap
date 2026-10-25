import re

path = r'D:\Projects\Ferrotap\src\ui.rs'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()

content = content.replace('crate::model::PausedEdits::default()', 'crate::model::BpEdit::default()')
content = content.replace('crate::model::PausedEdits', 'crate::model::BpEdit')
content = content.replace('&p.body_prefix', '&p.body')
content = content.replace('p.body_prefix', 'p.body')

with open(path, 'w', encoding='utf-8') as f:
    f.write(content)

print('Fixed ui.rs')
