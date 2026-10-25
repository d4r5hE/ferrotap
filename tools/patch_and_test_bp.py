import sys, os

with open(r'D:\Projects\Ferrotap\src\model.rs', 'r', encoding='utf-8') as f:
    model_content = f.read()

if 'pub fn bp_match_req' not in model_content:
    helpers = (
        "\npub fn global_breakpoints_lock() -> &'static std::sync::RwLock<Vec<BreakpointRule>> {\n"
        "    GLOBAL_BPS.get_or_init(|| std::sync::RwLock::new(Vec::new()))\n"
        "}\n\n"
        "pub fn set_global_breakpoints(rules: Vec<BreakpointRule>) {\n"
        "    if let Ok(mut g) = global_breakpoints_lock().write() {\n"
        "        *g = rules;\n"
        "    }\n"
        "}\n\n"
        "pub fn bp_match_req(method: &str, url: &str) -> Option<String> {\n"
        "    global_breakpoints_lock().read().ok().and_then(|g| bp_first_match(&g, BpStage::Request, method, url, 0))\n"
        "}\n\n"
        "pub fn bp_match_resp(method: &str, url: &str, status: u16) -> Option<String> {\n"
        "    global_breakpoints_lock().read().ok().and_then(|g| bp_first_match(&g, BpStage::Response, method, url, status))\n"
        "}\n"
    )
    if 'mod breakpoint_tests' in model_content:
        model_content = model_content.replace('mod breakpoint_tests', helpers + '\nmod breakpoint_tests')
    else:
        model_content += helpers
    with open(r'D:\Projects\Ferrotap\src\model.rs', 'w', encoding='utf-8') as f:
        f.write(model_content)
    print('Added bp helpers to model.rs')
else:
    print('model.rs already has bp helpers')
