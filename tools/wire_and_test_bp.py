import sys, os, subprocess, time, json

proxy_path = r'D:\Projects\Ferrotap\src\proxy.rs'
with open(proxy_path, 'r', encoding='utf-8') as f:
    p_code = f.read()

print('Read proxy.rs, length:', len(p_code))

# Let's see how rules_match is integrated in proxy.rs
idx = p_code.find('rules_match')
if idx != -1:
    print('Found rules_match around:', p_code[max(0, idx-200):min(len(p_code), idx+400)])
else:
    print('rules_match not found!')
