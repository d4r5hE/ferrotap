import sys, os, re

proxy_path = r'D:\Projects\Ferrotap\src\proxy.rs'
with open(proxy_path, 'r', encoding='utf-8') as f:
    p_src = f.read()

print('proxy.rs length:', len(p_src))
print('has rules_match:', 'rules_match' in p_src)
print('has breakpoint:', 'bp_' in p_src or 'breakpoint' in p_src.lower())
