//! Per-module UI state for the feature modules, so each can grow without touching GridApp's own fields.

#[derive(Default)]
pub struct ExtState {
    pub inspect: crate::inspect::State,
    pub data: crate::data_io::State,
    pub insights: crate::insights::State,
    pub rules: crate::rules_ext::State,
    pub api: crate::api_tools::State,
    pub streams: crate::streams::State,
    pub llm: crate::llm_traffic::State,
    pub auto: crate::automation::State,
    pub qol: crate::qol::State,
    /// G1: every selected session (model.selected is the focused one)
    pub sel: crate::selection::Selection,
    pub compare: crate::compare::CompareState,
    /// Shared in-app file picker (rules files, response files...)
    pub pick: crate::filepick::FilePicker,
    /// G8 Connections window
    pub conn: crate::conn::ConnUi,
    /// Advanced replay window
    pub replay: crate::replay_adv::ReplayUi,
    /// Intercept window (browsers, terminals, programs) and the instances it started
    pub launch: crate::launch::LaunchUi,
    /// Edit-session window
    pub edit: crate::session_edit::EditUi,
    /// Composer cookies editor / raw params
    pub composer: crate::composer_more::ComposerMore,
    /// Composer collections tree (folders)
    pub tree: crate::composer_more::TreeUi,
    /// Protobuf settings (.proto files for gRPC)
    pub proto: crate::protoschema::ProtoUi,
    /// The inspector is shown in its own native window.
    pub popout_inspector: bool,
    /// Filter Builder row whose suggestions are open: (row, name box?).
    pub fb_suggest: Option<(usize, bool)>,
    /// Scripts window and the live script set
    pub scripts: crate::scripting::ScriptsUi,
    /// Packets window (pcap / pcapng files)
    pub packets: crate::pcap_ui::PacketsUi,
    /// Insights > Protocols window
    pub show_protocols: bool,
    /// Android (ADB) window
    pub android: crate::android::AndroidUi,
    /// Tool windows shown in their own native window (Windows menu)
    pub popped: std::collections::HashSet<String>,
    /// Assistant (local model) window
    pub ai: crate::ai_assist::AiUi,
    /// The grid's visible session ids in row order, from the last frame (for Shift+click and the Clear menu)
    pub last_visible: Vec<usize>,
    /// A Clear-menu choice that needs the visible rows: "selected", "visible" or "hidden"
    pub pending_clear: Option<&'static str>,
}
